use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::{self, Duration, MissedTickBehavior};

use crate::config::{
    ConfigUpdate, EdiDestination, InputConfig, InputTiming, SharedConfig, SubchannelConfig,
    ValidatedConfig,
};
use crate::fic::FicCarousel;
use crate::timing::{TaiClock, TaiSource};
use dabmux::edi::{
    decode_sti_payload, decode_sti_rtp, fragment_af, pointer_tag, AfPacket, Deti, Est,
    PftReassembler, TimedPayload,
};
use dabmux::frame::{assemble, mnsc, FrameClock, Stream, FRAME_PERIOD_MS};

#[derive(Default)]
pub struct RuntimeStats {
    pub generated_frames: AtomicU64,
    pub config_activations: AtomicU64,
    pub input_underflows: AtomicU64,
    pub input_drops: AtomicU64,
    pub decode_errors: AtomicU64,
    pub send_errors: AtomicU64,
    pub missed_ticks: AtomicU64,
    pub buffered_input_frames: AtomicU64,
    pub late_input_frames: AtomicU64,
    pub invalid_timestamps: AtomicU64,
}

impl RuntimeStats {
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            generated_frames: self.generated_frames.load(Ordering::Relaxed),
            config_activations: self.config_activations.load(Ordering::Relaxed),
            input_underflows: self.input_underflows.load(Ordering::Relaxed),
            input_drops: self.input_drops.load(Ordering::Relaxed),
            decode_errors: self.decode_errors.load(Ordering::Relaxed),
            send_errors: self.send_errors.load(Ordering::Relaxed),
            missed_ticks: self.missed_ticks.load(Ordering::Relaxed),
            buffered_input_frames: self.buffered_input_frames.load(Ordering::Relaxed),
            late_input_frames: self.late_input_frames.load(Ordering::Relaxed),
            invalid_timestamps: self.invalid_timestamps.load(Ordering::Relaxed),
        }
    }
}

#[derive(serde::Serialize)]
pub struct StatsSnapshot {
    generated_frames: u64,
    config_activations: u64,
    input_underflows: u64,
    input_drops: u64,
    decode_errors: u64,
    send_errors: u64,
    missed_ticks: u64,
    buffered_input_frames: u64,
    late_input_frames: u64,
    invalid_timestamps: u64,
}

struct BufferedInput {
    rx: mpsc::Receiver<TimedPayload>,
    queue: VecDeque<TimedPayload>,
    max_frames: usize,
    prebuffer_frames: usize,
    prebuffering: bool,
    timing: InputTiming,
}

impl BufferedInput {
    fn new(rx: mpsc::Receiver<TimedPayload>, input: &InputConfig) -> Self {
        let (max_frames, prebuffer_frames, timing) = match input {
            InputConfig::Edi {
                buffer_frames,
                prebuffer_frames,
                timing,
                ..
            } => (*buffer_frames, *prebuffer_frames, *timing),
            InputConfig::Sti { .. } => (64, 1, InputTiming::Prebuffering),
        };
        Self {
            rx,
            queue: VecDeque::new(),
            max_frames,
            prebuffer_frames,
            prebuffering: true,
            timing,
        }
    }

    fn take(&mut self, clock: FrameClock, stats: &RuntimeStats) -> Option<Vec<u8>> {
        while let Ok(payload) = self.rx.try_recv() {
            if self.timing == InputTiming::Timestamped {
                let Some(timestamp) = payload_time_ms(&payload) else {
                    stats.invalid_timestamps.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let position = self
                    .queue
                    .iter()
                    .position(|item| payload_time_ms(item).is_some_and(|time| time >= timestamp))
                    .unwrap_or(self.queue.len());
                if self
                    .queue
                    .get(position)
                    .and_then(payload_time_ms)
                    .is_some_and(|time| time == timestamp)
                {
                    stats.input_drops.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                self.queue.insert(position, payload);
            } else {
                self.queue.push_back(payload);
            }
            if self.queue.len() > self.max_frames {
                self.queue.pop_front();
                stats.input_drops.fetch_add(1, Ordering::Relaxed);
            }
        }
        match self.timing {
            InputTiming::Prebuffering => {
                if self.prebuffering && self.queue.len() < self.prebuffer_frames {
                    return None;
                }
                self.prebuffering = false;
                match self.queue.pop_front() {
                    Some(payload) => Some(payload.bytes),
                    None => {
                        self.prebuffering = true;
                        None
                    }
                }
            }
            InputTiming::Timestamped => {
                let target = clock.unix_seconds * 1000 + i64::from(clock.millisecond);
                loop {
                    let next = self.queue.front()?;
                    let timestamp = match payload_time_ms(next) {
                        Some(time) => time,
                        None => {
                            self.queue.pop_front();
                            stats.invalid_timestamps.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    };
                    let difference = target - timestamp;
                    if difference < 0 {
                        return None;
                    }
                    if difference >= 24 {
                        self.queue.pop_front();
                        stats.late_input_frames.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    return self.queue.pop_front().map(|payload| payload.bytes);
                }
            }
        }
    }
}

fn payload_time_ms(payload: &TimedPayload) -> Option<i64> {
    let (Some(utco), Some(seconds), Some(tsta)) = (payload.utco, payload.seconds, payload.tsta)
    else {
        return None;
    };
    if tsta >= 0xfa_0000 {
        return None;
    }
    let seconds = i64::from(seconds) - i64::from(utco) + 946_684_800;
    Some(seconds * 1000 + i64::from(tsta) * 1000 / 16_384_000)
}

fn bind_address(input: &InputConfig) -> Result<String> {
    let address = match input {
        InputConfig::Edi { uri, .. } => uri
            .strip_prefix("udp://")
            .or_else(|| uri.strip_prefix("tcp://")),
        InputConfig::Sti { uri } => uri.strip_prefix("rtp://"),
    }
    .context("input URI has unsupported transport")?;
    let address = if address.starts_with(':') {
        format!("0.0.0.0{address}")
    } else {
        address.to_owned()
    };
    address
        .parse::<std::net::SocketAddr>()
        .context("input URI must contain an IP address and port")?;
    Ok(address)
}

async fn receive_input_tcp(
    listener: Arc<TcpListener>,
    stream_index: u16,
    tx: mpsc::Sender<TimedPayload>,
    stats: Arc<RuntimeStats>,
) -> Result<()> {
    let mut clients = tokio::task::JoinSet::new();
    loop {
        let (mut stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = clients.join_next(), if !clients.is_empty() => continue,
        };
        let tx = tx.clone();
        let stats = stats.clone();
        clients.spawn(async move {
            loop {
                let mut header = [0u8; 10];
                if stream.read_exact(&mut header).await.is_err() {
                    break;
                }
                if &header[..2] != b"AF" {
                    break;
                }
                let len =
                    u32::from_be_bytes(header[2..6].try_into().expect("fixed header")) as usize;
                if len > 64 * 1024 {
                    break;
                }
                let mut packet = header.to_vec();
                packet.resize(12 + len, 0);
                if stream.read_exact(&mut packet[10..]).await.is_err() {
                    break;
                }
                match AfPacket::decode(&packet).and_then(|p| decode_sti_payload(&p, stream_index)) {
                    Ok(payload) => {
                        if tx.try_send(payload).is_err() {
                            stats.input_drops.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(err) => {
                        stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                        tracing::debug!(%peer, %err, "invalid TCP EDI packet");
                    }
                }
            }
        });
    }
}

async fn serve_edi_tcp(
    listener: Arc<TcpListener>,
    state: Arc<StdMutex<TcpOutputState>>,
    stats: Arc<RuntimeStats>,
) -> Result<()> {
    let mut clients = tokio::task::JoinSet::new();
    loop {
        let (mut client, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = clients.join_next(), if !clients.is_empty() => continue,
        };
        let mut rx = {
            let mut state = state.lock().expect("TCP output state poisoned");
            let (tx, rx) = mpsc::channel(state.max_queue);
            for packet in &state.history {
                tx.try_send(packet.clone())
                    .expect("validated preroll capacity");
            }
            state.clients.push(tx);
            rx
        };
        let stats = stats.clone();
        clients.spawn(async move {
            while let Some(packet) = rx.recv().await {
                if let Err(err) = client.write_all(&packet).await {
                    stats.send_errors.fetch_add(1, Ordering::Relaxed);
                    tracing::debug!(%peer, %err, "EDI TCP client disconnected");
                    break;
                }
            }
        });
    }
}

async fn receive_input(
    input: InputConfig,
    socket: Arc<UdpSocket>,
    tx: mpsc::Sender<TimedPayload>,
    stats: Arc<RuntimeStats>,
) -> Result<()> {
    let address = socket.local_addr()?;
    let stream_index = match &input {
        InputConfig::Edi { stream_index, .. } => *stream_index,
        InputConfig::Sti { .. } => 1,
    };
    let mut buf = vec![0u8; 65536];
    let mut pft = PftReassembler::default();
    loop {
        let (size, _) = socket.recv_from(&mut buf).await?;
        let payload = match &input {
            InputConfig::Edi { .. } => {
                let packet = if buf[..size].starts_with(b"PF") {
                    pft.push(&buf[..size])
                } else {
                    AfPacket::decode(&buf[..size]).map(Some)
                };
                packet.map(|packet| packet.map(|p| decode_sti_payload(&p, stream_index)))
            }
            InputConfig::Sti { .. } => Ok(Some(decode_sti_rtp(&buf[..size]))),
        };
        match payload {
            Ok(Some(Ok(payload))) => {
                if tx.try_send(payload).is_err() {
                    stats.input_drops.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(Some(Err(err))) | Err(err) => {
                stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                tracing::debug!(%address, %err, "invalid input packet");
            }
            Ok(None) => {}
        }
    }
}

#[derive(Clone)]
enum InputSocket {
    Udp(Arc<UdpSocket>),
    Tcp(Arc<TcpListener>),
}

struct InputHandle {
    key: String,
    config: SubchannelConfig,
    socket: InputSocket,
    task: tokio::task::JoinHandle<()>,
    buffered: BufferedInput,
}

impl Drop for InputHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct TcpOutputHandle {
    port: u16,
    listener: Arc<TcpListener>,
    max_frames_queued: usize,
    preroll_ms: u32,
    state: Arc<StdMutex<TcpOutputState>>,
    task: tokio::task::JoinHandle<()>,
}

struct TcpOutputState {
    clients: Vec<mpsc::Sender<Arc<Vec<u8>>>>,
    history: VecDeque<Arc<Vec<u8>>>,
    preroll_frames: usize,
    max_queue: usize,
}

impl TcpOutputHandle {
    fn send(&self, packet: Arc<Vec<u8>>, stats: &RuntimeStats) {
        let mut state = self.state.lock().expect("TCP output state poisoned");
        if state.preroll_frames > 0 {
            state.history.push_back(packet.clone());
            while state.history.len() > state.preroll_frames {
                state.history.pop_front();
            }
        }
        state.clients.retain(|client| {
            if client.try_send(packet.clone()).is_ok() {
                true
            } else {
                stats.send_errors.fetch_add(1, Ordering::Relaxed);
                false
            }
        });
    }
}

impl Drop for TcpOutputHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Default)]
struct PreparedResources {
    inputs: HashMap<String, InputSocket>,
    outputs: HashMap<u16, Arc<TcpListener>>,
    tai_offset: u8,
}

fn input_key(input: &InputConfig) -> Result<String> {
    let transport = match input {
        InputConfig::Edi { uri, .. } if uri.starts_with("tcp://") => "tcp",
        InputConfig::Edi { .. } | InputConfig::Sti { .. } => "udp",
    };
    let address: std::net::SocketAddr = bind_address(input)?.parse()?;
    Ok(format!("{transport}://{address}"))
}

async fn prepare_resources(
    candidate: &ValidatedConfig,
    existing_inputs: &HashSet<String>,
    existing_outputs: &HashSet<u16>,
    current_tai: Option<(TaiSource, u8)>,
) -> Result<PreparedResources> {
    let mut prepared = PreparedResources::default();
    let tai_source = TaiSource::from_config(&candidate.source.ensemble);
    prepared.tai_offset = if let Some((source, offset)) = current_tai {
        if source == tai_source {
            offset
        } else {
            tai_source.resolve().await?
        }
    } else {
        tai_source.resolve().await?
    };
    for sub in &candidate.source.subchannels {
        let key = input_key(&sub.input)?;
        if existing_inputs.contains(&key) || prepared.inputs.contains_key(&key) {
            continue;
        }
        let address = bind_address(&sub.input)?;
        let socket = if key.starts_with("tcp://") {
            InputSocket::Tcp(Arc::new(
                TcpListener::bind(&address)
                    .await
                    .with_context(|| format!("binding EDI TCP input {address}"))?,
            ))
        } else {
            InputSocket::Udp(Arc::new(
                UdpSocket::bind(&address)
                    .await
                    .with_context(|| format!("binding input {address}"))?,
            ))
        };
        prepared.inputs.insert(key, socket);
    }
    for destination in &candidate.source.output.destinations {
        if let EdiDestination::Tcp { listen_port, .. } = destination {
            if existing_outputs.contains(listen_port) || prepared.outputs.contains_key(listen_port)
            {
                continue;
            }
            prepared.outputs.insert(
                *listen_port,
                Arc::new(
                    TcpListener::bind(("0.0.0.0", *listen_port))
                        .await
                        .with_context(|| format!("binding EDI TCP output {listen_port}"))?,
                ),
            );
        }
    }
    Ok(prepared)
}

fn start_input(
    subchannel: SubchannelConfig,
    socket: InputSocket,
    stats: Arc<RuntimeStats>,
) -> InputHandle {
    let input = subchannel.input.clone();
    let key = input_key(&input).expect("validated input URI");
    let buffer_size = match &input {
        InputConfig::Edi { buffer_frames, .. } => *buffer_frames,
        InputConfig::Sti { .. } => 64,
    };
    let (tx, rx) = mpsc::channel(buffer_size);
    let buffered = BufferedInput::new(rx, &input);
    let task = match socket.clone() {
        InputSocket::Tcp(listener) => {
            let stream_index = match &input {
                InputConfig::Edi { stream_index, .. } => *stream_index,
                _ => unreachable!(),
            };
            tokio::spawn(async move {
                if let Err(err) = receive_input_tcp(listener, stream_index, tx, stats).await {
                    tracing::error!(%err, "EDI TCP receiver stopped");
                }
            })
        }
        InputSocket::Udp(socket) => tokio::spawn(async move {
            if let Err(err) = receive_input(input, socket, tx, stats).await {
                tracing::error!(%err, "input receiver stopped");
            }
        }),
    };
    InputHandle {
        key,
        config: subchannel,
        socket,
        task,
        buffered,
    }
}

fn start_tcp_output(
    port: u16,
    listener: Arc<TcpListener>,
    max_frames_queued: usize,
    preroll_ms: u32,
    stats: Arc<RuntimeStats>,
) -> TcpOutputHandle {
    let state = Arc::new(StdMutex::new(TcpOutputState {
        clients: Vec::new(),
        history: VecDeque::new(),
        preroll_frames: preroll_ms.div_ceil(FRAME_PERIOD_MS as u32) as usize,
        max_queue: max_frames_queued,
    }));
    let task_state = state.clone();
    let task_listener = listener.clone();
    let task = tokio::spawn(async move {
        if let Err(err) = serve_edi_tcp(task_listener, task_state, stats).await {
            tracing::error!(%err, "EDI TCP server stopped");
        }
    });
    TcpOutputHandle {
        port,
        listener,
        max_frames_queued,
        preroll_ms,
        state,
        task,
    }
}

fn commit_resources(
    candidate: &ValidatedConfig,
    mut prepared: PreparedResources,
    inputs: &mut Vec<InputHandle>,
    outputs: &mut Vec<TcpOutputHandle>,
    udp_destinations: &mut Vec<String>,
    stats: &Arc<RuntimeStats>,
) {
    let mut next_inputs = Vec::with_capacity(candidate.source.subchannels.len());
    for sub in &candidate.source.subchannels {
        let key = input_key(&sub.input).expect("validated input URI");
        if let Some(position) = inputs
            .iter()
            .position(|old| old.key == key && old.config == *sub)
        {
            next_inputs.push(inputs.swap_remove(position));
            continue;
        }
        let socket = if let Some(position) = inputs.iter().position(|old| old.key == key) {
            let old = inputs.swap_remove(position);
            let socket = old.socket.clone();
            drop(old);
            socket
        } else {
            prepared.inputs.remove(&key).expect("prepared input socket")
        };
        next_inputs.push(start_input(sub.clone(), socket, stats.clone()));
    }
    *inputs = next_inputs;

    let mut next_outputs = Vec::new();
    let mut next_udp = Vec::new();
    for destination in &candidate.source.output.destinations {
        match destination {
            EdiDestination::Udp { address, port } => next_udp.push(format!("{address}:{port}")),
            EdiDestination::Tcp {
                listen_port,
                max_frames_queued,
                preroll_ms,
            } => {
                if let Some(position) = outputs.iter().position(|old| {
                    old.port == *listen_port
                        && old.max_frames_queued == *max_frames_queued
                        && old.preroll_ms == *preroll_ms
                }) {
                    next_outputs.push(outputs.swap_remove(position));
                } else {
                    let listener = if let Some(position) =
                        outputs.iter().position(|old| old.port == *listen_port)
                    {
                        let old = outputs.swap_remove(position);
                        let listener = old.listener.clone();
                        drop(old);
                        listener
                    } else {
                        prepared
                            .outputs
                            .remove(listen_port)
                            .expect("prepared EDI output")
                    };
                    next_outputs.push(start_tcp_output(
                        *listen_port,
                        listener,
                        *max_frames_queued,
                        *preroll_ms,
                        stats.clone(),
                    ));
                }
            }
        }
    }
    *outputs = next_outputs;
    *udp_destinations = next_udp;
}

pub async fn run(config: SharedConfig, stats: Arc<RuntimeStats>) -> Result<()> {
    let initial = config.read().await.clone();
    let prepared = prepare_resources(&initial, &HashSet::new(), &HashSet::new(), None).await?;
    let mut tai_clock = TaiClock::new(
        TaiSource::from_config(&initial.source.ensemble),
        prepared.tai_offset,
    );
    let mut receivers = Vec::new();
    let mut tcp_outputs = Vec::new();
    let mut udp_destinations = Vec::new();
    commit_resources(
        &initial,
        prepared,
        &mut receivers,
        &mut tcp_outputs,
        &mut udp_destinations,
        &stats,
    );
    let sender = UdpSocket::bind("0.0.0.0:0").await?;
    let (update_tx, mut updates) = mpsc::channel::<ConfigUpdate>(1);
    let (prepared_tx, mut prepared_rx) = mpsc::channel(1);
    config.install_updates(update_tx).await;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let shifted_now = i64::try_from(now.as_millis())?
        .checked_add(i64::from(initial.source.ensemble.tist_offset_ms))
        .context("TIST startup offset overflow")?;
    let mut clock = FrameClock::from_wall_time(
        u64::try_from(shifted_now)?,
        initial.source.ensemble.tist_at_fct0_ms,
    )?;
    let mut carousel = FicCarousel::new();
    let mut seq = 0u16;
    let mut tick = time::interval(Duration::from_millis(FRAME_PERIOD_MS));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut previous_tick = None;
    let mut active = initial;
    let mut pending: Option<(ConfigUpdate, PreparedResources)> = None;

    loop {
        let instant = tokio::select! {
            instant = tick.tick() => instant,
            Some(update) = updates.recv() => {
                let input_keys: HashSet<_> = receivers.iter().map(|input: &InputHandle| input.key.clone()).collect();
                let output_ports: HashSet<_> = tcp_outputs.iter().map(|output: &TcpOutputHandle| output.port).collect();
                let prepared_tx = prepared_tx.clone();
                let current_tai = Some((tai_clock.source.clone(), tai_clock.offset()));
                tokio::spawn(async move {
                    let result = prepare_resources(&update.candidate, &input_keys, &output_ports, current_tai).await;
                    let _ = prepared_tx.send((update, result)).await;
                });
                continue;
            }
            Some((update, result)) = prepared_rx.recv() => {
                match result {
                    Ok(resources) => pending = Some((update, resources)),
                    Err(err) => { let _ = update.reply.send(Err(err)); }
                }
                continue;
            }
        };
        if let Some(previous) = previous_tick {
            let elapsed_ticks = (instant.duration_since(previous).as_micros()
                / u128::from(FRAME_PERIOD_MS * 1000))
            .max(1);
            let skipped = elapsed_ticks.saturating_sub(1);
            if skipped > 0 {
                stats
                    .missed_ticks
                    .fetch_add(skipped as u64, Ordering::Relaxed);
                for _ in 0..skipped {
                    clock.tick();
                }
            }
        }
        previous_tick = Some(instant);
        if let Some((update, resources)) = pending.take() {
            let candidate = update.candidate;
            let new_tai_source = TaiSource::from_config(&candidate.source.ensemble);
            let new_tai_offset = resources.tai_offset;
            clock.shift_millis(
                i64::from(candidate.source.ensemble.tist_offset_ms)
                    - i64::from(active.source.ensemble.tist_offset_ms),
            )?;
            if candidate.source.ensemble.tist_at_fct0_ms != active.source.ensemble.tist_at_fct0_ms {
                clock.rephase_fct0(candidate.source.ensemble.tist_at_fct0_ms)?;
            }
            commit_resources(
                &candidate,
                resources,
                &mut receivers,
                &mut tcp_outputs,
                &mut udp_destinations,
                &stats,
            );
            if tai_clock.source != new_tai_source {
                tai_clock = TaiClock::new(new_tai_source, new_tai_offset);
            }
            active = candidate.clone();
            carousel = FicCarousel::new();
            config.commit(candidate).await;
            stats.config_activations.fetch_add(1, Ordering::Relaxed);
            let _ = update.reply.send(Ok(true));
            tracing::info!("complete mux configuration activated");
        }
        let current = &active;
        let fic = carousel.write(current, clock)?;
        let mut payloads = Vec::with_capacity(current.subchannels.len());
        for (sub, input) in current.subchannels.iter().zip(&mut receivers) {
            let data = match input.buffered.take(clock, &stats) {
                Some(data) if data.len() == sub.payload_bytes => data,
                Some(_) | None => {
                    stats.input_underflows.fetch_add(1, Ordering::Relaxed);
                    vec![0u8; sub.payload_bytes]
                }
            };
            payloads.push(data);
        }
        stats.buffered_input_frames.store(
            receivers
                .iter()
                .map(|input| input.buffered.queue.len() as u64)
                .sum(),
            Ordering::Relaxed,
        );
        let streams: Vec<_> = current
            .subchannels
            .iter()
            .zip(&payloads)
            .map(|(sub, bytes)| Stream {
                id: sub.id,
                start_address_cu: sub.start_address_cu,
                tpl: sub.tpl,
                payload: bytes,
            })
            .collect();
        let tist = current.source.ensemble.tist;
        let frame = assemble(
            current.source.ensemble.mode,
            clock,
            mnsc(clock.unix_seconds, clock.fp()),
            &fic,
            &streams,
            tist,
        )?;
        let mid = match current.source.ensemble.mode {
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 0,
            _ => unreachable!(),
        };
        let timestamp = if tist {
            let tai_offset = tai_clock.offset();
            let utco = tai_offset - 32;
            let seconds = clock
                .unix_seconds
                .checked_sub(946_684_800)
                .and_then(|s| s.checked_add(i64::from(utco)))
                .context("EDI seconds overflow")?;
            Some((utco, u32::try_from(seconds)?, clock.tsta()))
        } else {
            None
        };
        let mut tags = vec![
            pointer_tag(*b"DETI"),
            Deti {
                dlfc: clock.dlfc(),
                stat: 0xff,
                mid,
                fp: clock.fp(),
                mnsc: frame.mnsc,
                timestamp,
                fic: &frame.fic,
            }
            .tag()?,
        ];
        for (index, (sub, bytes)) in current.subchannels.iter().zip(&payloads).enumerate() {
            tags.push(
                Est {
                    index: u8::try_from(index + 1)?,
                    scid: sub.id,
                    start_address: sub.start_address_cu,
                    tpl: sub.tpl,
                    payload: bytes,
                }
                .tag()?,
            );
        }
        let packet = AfPacket {
            sequence: seq,
            tags,
        }
        .encode_with_alignment(current.source.output.tagpacket_alignment)?;
        let fragments = fragment_af(&packet, seq)?;
        for destination in &udp_destinations {
            for fragment in &fragments {
                if let Err(err) = sender.send_to(fragment, destination).await {
                    stats.send_errors.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(%destination, %err, "EDI send failed");
                }
            }
        }
        let tcp_packet = Arc::new(packet.clone());
        for output in &tcp_outputs {
            output.send(tcp_packet.clone(), &stats);
        }
        seq = seq.wrapping_add(1);
        clock.tick();
        stats.generated_frames.fetch_add(1, Ordering::Relaxed);
        if receivers.len() != current.subchannels.len() {
            bail!("active subchannel count changed unexpectedly");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(bytes: Vec<u8>, seconds: Option<u32>, tsta: Option<u32>) -> TimedPayload {
        TimedPayload {
            dlfc: 0,
            utco: seconds.map(|_| 0),
            seconds,
            tsta,
            stream_index: 1,
            bytes,
        }
    }

    #[tokio::test]
    async fn prebuffering_and_underflow_recovery() {
        let (tx, rx) = mpsc::channel(8);
        let mut input = BufferedInput {
            rx,
            queue: VecDeque::new(),
            max_frames: 8,
            prebuffer_frames: 2,
            prebuffering: true,
            timing: InputTiming::Prebuffering,
        };
        let stats = RuntimeStats::default();
        let clock = FrameClock::new(0, 0, 0).unwrap();
        tx.send(payload(vec![1], None, None)).await.unwrap();
        assert_eq!(input.take(clock, &stats), None);
        tx.send(payload(vec![2], None, None)).await.unwrap();
        assert_eq!(input.take(clock, &stats), Some(vec![1]));
        assert_eq!(input.take(clock, &stats), Some(vec![2]));
        assert_eq!(input.take(clock, &stats), None);
        tx.send(payload(vec![3], None, None)).await.unwrap();
        assert_eq!(input.take(clock, &stats), None);
    }

    #[tokio::test]
    async fn timestamped_input_discards_late_and_keeps_future() {
        let (tx, rx) = mpsc::channel(8);
        let mut input = BufferedInput {
            rx,
            queue: VecDeque::new(),
            max_frames: 8,
            prebuffer_frames: 1,
            prebuffering: false,
            timing: InputTiming::Timestamped,
        };
        let stats = RuntimeStats::default();
        let clock = FrameClock::new(0, 946_684_800, 48).unwrap();
        tx.send(payload(vec![1], Some(0), Some(0))).await.unwrap();
        tx.send(payload(vec![2], Some(0), Some(48 * 16_384)))
            .await
            .unwrap();
        tx.send(payload(vec![3], Some(0), Some(96 * 16_384)))
            .await
            .unwrap();
        assert_eq!(input.take(clock, &stats), Some(vec![2]));
        assert_eq!(stats.late_input_frames.load(Ordering::Relaxed), 1);
        assert_eq!(input.take(clock, &stats), None);
        assert_eq!(input.queue.len(), 1);
    }

    #[tokio::test]
    async fn timestamped_input_orders_and_deduplicates() {
        let (tx, rx) = mpsc::channel(8);
        let mut input = BufferedInput {
            rx,
            queue: VecDeque::new(),
            max_frames: 8,
            prebuffer_frames: 1,
            prebuffering: false,
            timing: InputTiming::Timestamped,
        };
        let stats = RuntimeStats::default();
        let clock = FrameClock::new(0, 946_684_800, 48).unwrap();
        tx.send(payload(vec![3], Some(0), Some(96 * 16_384)))
            .await
            .unwrap();
        tx.send(payload(vec![2], Some(0), Some(48 * 16_384)))
            .await
            .unwrap();
        tx.send(payload(vec![9], Some(0), Some(48 * 16_384)))
            .await
            .unwrap();
        assert_eq!(input.take(clock, &stats), Some(vec![2]));
        assert_eq!(stats.input_drops.load(Ordering::Relaxed), 1);
        assert_eq!(input.queue.len(), 1);
    }
}
