use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::{self, Duration, MissedTickBehavior};

use crate::config::{
    ConfigUpdate, EdiDestination, InputConfig, InputEndpoint, InputTiming, SharedConfig,
    SubchannelConfig, SubchannelKind, Transport, ValidatedConfig,
};
use crate::fic::FicCarousel;
use crate::timing::{TaiClock, TaiSource};
use dabmux::edi::{
    decode_sti_payload, decode_sti_rtp, fragment_af, pointer_tag, AfPacket, Deti, Est,
    PftReassembler, TimedPayload,
};
use dabmux::frame::{
    assemble, cifs_per_transmission_frame, mnsc, FrameClock, Stream, FRAME_PERIOD_MS,
};

/// Close a TCP producer that sends nothing for this long (C++ uses 10 s too).
const TCP_INPUT_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Warn when the frame clock and system time disagree by more than this.
const CLOCK_DRIFT_WARN_MS: i64 = 100;
/// Compare the frame clock with system time once per 250 frames (6 s).
const CLOCK_CHECK_FRAMES: u64 = 250;
/// Warn when an input overflows this many frames within one check period.
const DROP_WARN_FRAMES: u64 = 10;

#[derive(Default)]
pub struct RuntimeStats {
    pub generated_frames: AtomicU64,
    pub config_activations: AtomicU64,
    pub input_underflows: AtomicU64,
    pub input_drops: AtomicU64,
    pub input_size_mismatches: AtomicU64,
    pub decode_errors: AtomicU64,
    pub send_errors: AtomicU64,
    pub missed_ticks: AtomicU64,
    pub buffered_input_frames: AtomicU64,
    pub late_input_frames: AtomicU64,
    pub invalid_timestamps: AtomicU64,
    pub frame_errors: AtomicU64,
    pub clock_drift_ms: AtomicI64,
}

impl RuntimeStats {
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            generated_frames: self.generated_frames.load(Ordering::Relaxed),
            config_activations: self.config_activations.load(Ordering::Relaxed),
            input_underflows: self.input_underflows.load(Ordering::Relaxed),
            input_drops: self.input_drops.load(Ordering::Relaxed),
            input_size_mismatches: self.input_size_mismatches.load(Ordering::Relaxed),
            decode_errors: self.decode_errors.load(Ordering::Relaxed),
            send_errors: self.send_errors.load(Ordering::Relaxed),
            missed_ticks: self.missed_ticks.load(Ordering::Relaxed),
            buffered_input_frames: self.buffered_input_frames.load(Ordering::Relaxed),
            late_input_frames: self.late_input_frames.load(Ordering::Relaxed),
            invalid_timestamps: self.invalid_timestamps.load(Ordering::Relaxed),
            frame_errors: self.frame_errors.load(Ordering::Relaxed),
            clock_drift_ms: self.clock_drift_ms.load(Ordering::Relaxed),
        }
    }
}

#[derive(serde::Serialize)]
pub struct StatsSnapshot {
    generated_frames: u64,
    config_activations: u64,
    input_underflows: u64,
    input_drops: u64,
    input_size_mismatches: u64,
    decode_errors: u64,
    send_errors: u64,
    missed_ticks: u64,
    buffered_input_frames: u64,
    late_input_frames: u64,
    invalid_timestamps: u64,
    frame_errors: u64,
    clock_drift_ms: i64,
}

struct BufferedInput {
    rx: mpsc::Receiver<TimedPayload>,
    queue: VecDeque<TimedPayload>,
    max_frames: usize,
    prebuffer_frames: usize,
    prebuffering: bool,
    timing: InputTiming,
    backpressure: bool,
    /// Frames discarded because the producer ran ahead of real time.
    overflow_drops: u64,
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
            backpressure: input.backpressure(),
            overflow_drops: 0,
        }
    }

    fn take(&mut self, clock: FrameClock, stats: &RuntimeStats) -> Option<Vec<u8>> {
        while !self.backpressure || self.queue.len() < self.max_frames {
            let Ok(payload) = self.rx.try_recv() else {
                break;
            };
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
                self.overflow_drops += 1;
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

async fn read_af_packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 10];
    stream.read_exact(&mut header).await?;
    if &header[..2] != b"AF" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stream is not AF framed",
        ));
    }
    let len = u32::from_be_bytes(header[2..6].try_into().expect("fixed header")) as usize;
    if len > 64 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "AF packet too large",
        ));
    }
    let mut packet = header.to_vec();
    packet.resize(12 + len, 0);
    stream.read_exact(&mut packet[10..]).await?;
    Ok(packet)
}

async fn receive_input_tcp(
    listener: Arc<TcpListener>,
    stream_index: u16,
    expected_bytes: usize,
    backpressure: bool,
    tx: mpsc::Sender<TimedPayload>,
    stats: Arc<RuntimeStats>,
) -> Result<()> {
    let mut clients = tokio::task::JoinSet::new();
    let active_client = Arc::new(AtomicU64::new(0));
    let mismatch_warned = Arc::new(AtomicBool::new(false));
    let mut next_client_id = 1u64;
    loop {
        let (mut stream, peer) = tokio::select! {
            accepted = listener.accept() => accepted?,
            _ = clients.join_next(), if !clients.is_empty() => continue,
        };
        let tx = tx.clone();
        let stats = stats.clone();
        let active_client = active_client.clone();
        let mismatch_warned = mismatch_warned.clone();
        let client_id = next_client_id;
        next_client_id = next_client_id.wrapping_add(1).max(1);
        clients.spawn(async move {
            let mut claimed = false;
            loop {
                let packet =
                    match time::timeout(TCP_INPUT_IDLE_TIMEOUT, read_af_packet(&mut stream)).await {
                        Ok(Ok(packet)) => packet,
                        Ok(Err(err)) => {
                            tracing::debug!(%peer, %err, "EDI TCP producer disconnected");
                            break;
                        }
                        Err(_) => {
                            tracing::info!(%peer, "EDI TCP producer idle; closing connection");
                            break;
                        }
                    };
                match AfPacket::decode(&packet).and_then(|p| decode_sti_payload(&p, stream_index)) {
                    Ok(payload) => {
                        if payload.bytes.len() != expected_bytes {
                            stats
                                .input_size_mismatches
                                .fetch_add(1, Ordering::Relaxed);
                            if !mismatch_warned.swap(true, Ordering::Relaxed) {
                                tracing::warn!(%peer, received_bytes = payload.bytes.len(), expected_bytes, "TCP EDI producer bitrate does not match subchannel; closing connection");
                            }
                            break;
                        }
                        // The newest producer wins: a restarted encoder reconnects
                        // before a dead connection is noticed, and must not be
                        // locked out by it.
                        if !claimed {
                            if active_client.swap(client_id, Ordering::AcqRel) != 0 {
                                tracing::info!(%peer, "EDI TCP producer replaces previous connection");
                            }
                            claimed = true;
                        } else if active_client.load(Ordering::Acquire) != client_id {
                            tracing::info!(%peer, "EDI TCP producer superseded by a newer connection");
                            break;
                        }
                        if backpressure {
                            if tx.send(payload).await.is_err() {
                                break;
                            }
                        } else {
                            match tx.try_send(payload) {
                                Ok(()) => {}
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    stats.input_drops.fetch_add(1, Ordering::Relaxed);
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => break,
                            }
                        }
                    }
                    Err(err) => {
                        stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                        tracing::debug!(%peer, %err, "invalid TCP EDI packet");
                    }
                }
            }
            let _ = active_client.compare_exchange(
                client_id,
                0,
                Ordering::AcqRel,
                Ordering::Relaxed,
            );
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
    expected_bytes: usize,
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
    let mut mismatch_warned = false;
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
                if payload.bytes.len() != expected_bytes {
                    stats.input_size_mismatches.fetch_add(1, Ordering::Relaxed);
                    if !mismatch_warned {
                        tracing::warn!(%address, received_bytes = payload.bytes.len(), expected_bytes, "input bitrate does not match subchannel; discarding frame");
                        mismatch_warned = true;
                    }
                    continue;
                }
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
    endpoint: InputEndpoint,
    config: SubchannelConfig,
    socket: InputSocket,
    task: tokio::task::JoinHandle<()>,
    buffered: BufferedInput,
    underflowing: bool,
    reported_drops: u64,
}

impl InputHandle {
    /// Only the input settings and the frame size matter to a running
    /// receiver; ID, protection and uid changes are applied in place.
    fn serves(&self, endpoint: InputEndpoint, sub: &SubchannelConfig) -> bool {
        self.endpoint == endpoint
            && self.config.input == sub.input
            && self.config.bitrate == sub.bitrate
    }
}

impl Drop for InputHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct TcpOutputHandle {
    port: u16,
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
    fn clear_history(&self) {
        self.state
            .lock()
            .expect("TCP output state poisoned")
            .history
            .clear();
    }

    /// Apply queue settings without disconnecting clients. A new queue size
    /// applies to clients that connect afterwards.
    fn reconfigure(&self, max_frames_queued: usize, preroll_ms: u32) {
        let mut state = self.state.lock().expect("TCP output state poisoned");
        state.max_queue = max_frames_queued;
        state.preroll_frames = preroll_frames(preroll_ms);
        while state.history.len() > state.preroll_frames {
            state.history.pop_front();
        }
    }

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

fn preroll_frames(preroll_ms: u32) -> usize {
    preroll_ms.div_ceil(FRAME_PERIOD_MS as u32) as usize
}

#[derive(Default)]
struct PreparedResources {
    inputs: HashMap<InputEndpoint, InputSocket>,
    outputs: HashMap<u16, Arc<TcpListener>>,
    tai_offset: u8,
}

/// Explain a bind failure caused by the active configuration still holding the port.
fn bind_error(
    err: std::io::Error,
    what: &str,
    port: u16,
    held_ports: &HashSet<u16>,
) -> anyhow::Error {
    if err.kind() == std::io::ErrorKind::AddrInUse && held_ports.contains(&port) {
        anyhow::anyhow!(
            "binding {what}: port {port} is still held by the active configuration; \
             release it in one update and reuse it in a later one"
        )
    } else {
        anyhow::Error::new(err).context(format!("binding {what}"))
    }
}

async fn prepare_resources(
    candidate: &ValidatedConfig,
    existing_inputs: &HashSet<InputEndpoint>,
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
    let held_ports: HashSet<u16> = existing_inputs
        .iter()
        .map(|endpoint| endpoint.address.port())
        .chain(existing_outputs.iter().copied())
        .collect();
    for sub in &candidate.subchannels {
        let endpoint = sub.endpoint;
        if existing_inputs.contains(&endpoint) || prepared.inputs.contains_key(&endpoint) {
            continue;
        }
        let port = endpoint.address.port();
        let socket = match endpoint.transport {
            Transport::Tcp => InputSocket::Tcp(Arc::new(
                TcpListener::bind(endpoint.address).await.map_err(|err| {
                    bind_error(err, &format!("input {endpoint}"), port, &held_ports)
                })?,
            )),
            Transport::Udp => {
                InputSocket::Udp(Arc::new(UdpSocket::bind(endpoint.address).await.map_err(
                    |err| bind_error(err, &format!("input {endpoint}"), port, &held_ports),
                )?))
            }
        };
        prepared.inputs.insert(endpoint, socket);
    }
    for destination in &candidate.source.output.destinations {
        if let EdiDestination::Tcp { listen_port, .. } = destination {
            if existing_outputs.contains(listen_port) || prepared.outputs.contains_key(listen_port)
            {
                continue;
            }
            let listener = TcpListener::bind(("0.0.0.0", *listen_port))
                .await
                .map_err(|err| bind_error(err, "EDI TCP output", *listen_port, &held_ports))?;
            prepared.outputs.insert(*listen_port, Arc::new(listener));
        }
    }
    Ok(prepared)
}

fn start_input(
    subchannel: SubchannelConfig,
    endpoint: InputEndpoint,
    socket: InputSocket,
    stats: Arc<RuntimeStats>,
) -> InputHandle {
    let input = subchannel.input.clone();
    let backpressure = input.backpressure();
    // With backpressure the producer blocks once the playout buffer is full,
    // so the channel must not add a second buffer's worth of latency.
    let channel_capacity = match &input {
        _ if backpressure => 1,
        InputConfig::Edi { buffer_frames, .. } => *buffer_frames,
        InputConfig::Sti { .. } => 64,
    };
    let (tx, rx) = mpsc::channel(channel_capacity);
    let buffered = BufferedInput::new(rx, &input);
    let expected_bytes = subchannel.bitrate as usize * 3;
    let task = match socket.clone() {
        InputSocket::Tcp(listener) => {
            let stream_index = match &input {
                InputConfig::Edi { stream_index, .. } => *stream_index,
                _ => unreachable!(),
            };
            tokio::spawn(async move {
                if let Err(err) = receive_input_tcp(
                    listener,
                    stream_index,
                    expected_bytes,
                    backpressure,
                    tx,
                    stats,
                )
                .await
                {
                    tracing::error!(%err, "EDI TCP receiver stopped");
                }
            })
        }
        InputSocket::Udp(socket) => tokio::spawn(async move {
            if let Err(err) = receive_input(input, socket, expected_bytes, tx, stats).await {
                tracing::error!(%err, "input receiver stopped");
            }
        }),
    };
    InputHandle {
        endpoint,
        config: subchannel,
        socket,
        task,
        buffered,
        underflowing: false,
        reported_drops: 0,
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
        preroll_frames: preroll_frames(preroll_ms),
        max_queue: max_frames_queued,
    }));
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        if let Err(err) = serve_edi_tcp(listener, task_state, stats).await {
            tracing::error!(%err, "EDI TCP server stopped");
        }
    });
    TcpOutputHandle { port, state, task }
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
    for (sub, validated) in candidate
        .source
        .subchannels
        .iter()
        .zip(&candidate.subchannels)
    {
        let endpoint = validated.endpoint;
        if let Some(position) = inputs.iter().position(|old| old.serves(endpoint, sub)) {
            let mut handle = inputs.swap_remove(position);
            handle.config = sub.clone();
            next_inputs.push(handle);
            continue;
        }
        let socket = if let Some(position) = inputs.iter().position(|old| old.endpoint == endpoint)
        {
            let old = inputs.swap_remove(position);
            let socket = old.socket.clone();
            drop(old);
            socket
        } else {
            prepared
                .inputs
                .remove(&endpoint)
                .expect("prepared input socket")
        };
        next_inputs.push(start_input(sub.clone(), endpoint, socket, stats.clone()));
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
                if let Some(position) = outputs.iter().position(|old| old.port == *listen_port) {
                    let handle = outputs.swap_remove(position);
                    handle.reconfigure(*max_frames_queued, *preroll_ms);
                    next_outputs.push(handle);
                } else {
                    let listener = prepared
                        .outputs
                        .remove(listen_port)
                        .expect("prepared EDI output");
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

/// Multiplex configuration information receivers must reacquire on change:
/// ensemble ID (FIG 0/0), sub-channel organisation (FIG 0/1) and service
/// organisation (FIG 0/2).
type Mci = (
    u16,
    Vec<(u8, u16, u16, u8)>,
    Vec<(u32, Vec<(u8, u8, SubchannelKind)>)>,
);

fn mci(config: &ValidatedConfig) -> Mci {
    let subchannels = config
        .subchannels
        .iter()
        .map(|sub| (sub.id, sub.start_address_cu, sub.size_cu, sub.tpl))
        .collect();
    let services = config
        .source
        .services
        .iter()
        .enumerate()
        .map(|(index, service)| {
            let components = config
                .service_components(index)
                .map(|component| {
                    (
                        config.subchannels[component.subchannel].id,
                        component.scids,
                        config.source.subchannels[component.subchannel].kind.clone(),
                    )
                })
                .collect();
            (service.id, components)
        })
        .collect();
    (config.source.ensemble.id, subchannels, services)
}

/// FIG 0/7 count after an activation. A count set in the new configuration
/// wins; otherwise a structural change advances the signalled count.
fn next_reconfiguration_counter(
    active_configured: Option<u16>,
    candidate_configured: Option<u16>,
    signalled: Option<u16>,
    structure_changed: bool,
) -> Option<u16> {
    if candidate_configured != active_configured {
        candidate_configured
    } else if structure_changed {
        signalled.map(|count| (count + 1) % 1024)
    } else {
        signalled
    }
}

/// Build one EDI AF packet from the active configuration and input payloads.
fn build_packet(
    config: &ValidatedConfig,
    clock: FrameClock,
    fic: &[u8],
    payloads: &[Vec<u8>],
    tai_offset: u8,
    seq: u16,
) -> Result<Vec<u8>> {
    let streams: Vec<_> = config
        .subchannels
        .iter()
        .zip(payloads)
        .map(|(sub, bytes)| Stream {
            id: sub.id,
            start_address_cu: sub.start_address_cu,
            tpl: sub.tpl,
            payload: bytes,
        })
        .collect();
    let tist = config.source.ensemble.tist;
    let frame = assemble(
        config.source.ensemble.mode,
        clock,
        mnsc(clock.unix_seconds, clock.fp()),
        fic,
        &streams,
        tist,
    )?;
    let mid = match config.source.ensemble.mode {
        1 => 1,
        2 => 2,
        3 => 3,
        _ => 0,
    };
    let timestamp = if tist {
        let utco = tai_offset
            .checked_sub(32)
            .context("TAI-UTC offset below 32 s")?;
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
    for (index, (sub, bytes)) in config.subchannels.iter().zip(payloads).enumerate() {
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
    AfPacket {
        sequence: seq,
        tags,
    }
    .encode_with_alignment(config.source.output.tagpacket_alignment)
}

fn unix_millis_now() -> Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
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
    let shifted_now = unix_millis_now()?
        .checked_add(i64::from(initial.source.ensemble.tist_offset_ms))
        .context("TIST startup offset overflow")?;
    let mut clock = FrameClock::from_wall_time(
        u64::try_from(shifted_now)?,
        initial.source.ensemble.tist_at_fct0_ms,
    )?;
    let mut reconfiguration_counter = initial.source.ensemble.reconfiguration_counter;
    let mut carousel = FicCarousel::new().with_reconfiguration_counter(reconfiguration_counter);
    let mut seq = 0u16;
    let mut tick = time::interval(Duration::from_millis(FRAME_PERIOD_MS));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut previous_tick = None;
    let mut active = initial;
    let mut pending: Option<(ConfigUpdate, PreparedResources)> = None;
    let mut frame_failing = false;
    let mut drift_warned = false;

    loop {
        let instant = tokio::select! {
            instant = tick.tick() => instant,
            Some(update) = updates.recv() => {
                let input_endpoints: HashSet<_> = receivers.iter().map(|input: &InputHandle| input.endpoint).collect();
                let output_ports: HashSet<_> = tcp_outputs.iter().map(|output: &TcpOutputHandle| output.port).collect();
                let prepared_tx = prepared_tx.clone();
                let current_tai = Some((tai_clock.source.clone(), tai_clock.offset()));
                tokio::spawn(async move {
                    let result = prepare_resources(&update.candidate, &input_endpoints, &output_ports, current_tai).await;
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

        // Switch configurations only where a transmission frame begins.
        let frame_boundary = clock
            .count
            .is_multiple_of(cifs_per_transmission_frame(active.source.ensemble.mode));
        if let Some((update, resources)) = pending.take_if(|_| frame_boundary) {
            let candidate = update.candidate;
            let mut next_clock = clock;
            let clock_change = next_clock
                .shift_millis(
                    i64::from(candidate.source.ensemble.tist_offset_ms)
                        - i64::from(active.source.ensemble.tist_offset_ms),
                )
                .and_then(|()| {
                    if candidate.source.ensemble.tist_at_fct0_ms
                        != active.source.ensemble.tist_at_fct0_ms
                    {
                        next_clock.rephase_fct0(candidate.source.ensemble.tist_at_fct0_ms)
                    } else {
                        Ok(())
                    }
                });
            if let Err(err) = clock_change {
                let _ = update.reply.send(Err(err));
            } else {
                clock = next_clock;
                let structure_changed = mci(&candidate) != mci(&active);
                let tai_offset = resources.tai_offset;
                commit_resources(
                    &candidate,
                    resources,
                    &mut receivers,
                    &mut tcp_outputs,
                    &mut udp_destinations,
                    &stats,
                );
                reconfiguration_counter = next_reconfiguration_counter(
                    active.source.ensemble.reconfiguration_counter,
                    candidate.source.ensemble.reconfiguration_counter,
                    reconfiguration_counter,
                    structure_changed,
                );
                if structure_changed {
                    for output in &tcp_outputs {
                        output.clear_history();
                    }
                    tracing::warn!(
                        reconfiguration_counter,
                        "ensemble structure changed; TCP preroll cleared; receivers may need to reacquire"
                    );
                }
                let new_tai_source = TaiSource::from_config(&candidate.source.ensemble);
                if tai_clock.source != new_tai_source {
                    tai_clock = TaiClock::new(new_tai_source, tai_offset);
                }
                active = candidate.clone();
                carousel = FicCarousel::new().with_reconfiguration_counter(reconfiguration_counter);
                config.commit(candidate).await;
                stats.config_activations.fetch_add(1, Ordering::Relaxed);
                let _ = update.reply.send(Ok(true));
                tracing::info!("complete mux configuration activated");
            }
        }

        let mut payloads = Vec::with_capacity(active.subchannels.len());
        for (sub, input) in active.subchannels.iter().zip(&mut receivers) {
            let data = match input.buffered.take(clock, &stats) {
                Some(data) if data.len() == sub.payload_bytes => {
                    if input.underflowing {
                        tracing::info!(subchannel = %sub.uid, "input recovered");
                        input.underflowing = false;
                    }
                    data
                }
                data => {
                    stats.input_underflows.fetch_add(1, Ordering::Relaxed);
                    if !input.underflowing {
                        tracing::warn!(subchannel = %sub.uid, received_bytes = ?data.as_ref().map(Vec::len), expected_bytes = sub.payload_bytes, "input underflow; substituting silence");
                        input.underflowing = true;
                    }
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

        // A failed frame is skipped and counted; the mux keeps its timing.
        let packet = carousel.write(&active, clock).and_then(|fic| {
            let packet = build_packet(&active, clock, &fic, &payloads, tai_clock.offset(), seq)?;
            let fragments = fragment_af(&packet, seq)?;
            Ok((packet, fragments))
        });
        match packet {
            Ok((packet, fragments)) => {
                if frame_failing {
                    tracing::info!("frame generation recovered");
                    frame_failing = false;
                }
                for destination in &udp_destinations {
                    for fragment in &fragments {
                        if let Err(err) = sender.send_to(fragment, destination).await {
                            stats.send_errors.fetch_add(1, Ordering::Relaxed);
                            tracing::warn!(%destination, %err, "EDI send failed");
                        }
                    }
                }
                let tcp_packet = Arc::new(packet);
                for output in &tcp_outputs {
                    output.send(tcp_packet.clone(), &stats);
                }
                stats.generated_frames.fetch_add(1, Ordering::Relaxed);
            }
            Err(err) => {
                stats.frame_errors.fetch_add(1, Ordering::Relaxed);
                if !frame_failing {
                    tracing::error!(%err, "frame generation failed; skipping frame");
                    frame_failing = true;
                }
            }
        }

        if clock.count.is_multiple_of(CLOCK_CHECK_FRAMES) {
            for (sub, input) in active.subchannels.iter().zip(&mut receivers) {
                let dropped = input.buffered.overflow_drops - input.reported_drops;
                input.reported_drops = input.buffered.overflow_drops;
                if dropped >= DROP_WARN_FRAMES {
                    let hint = if sub.endpoint.transport == Transport::Tcp {
                        "enable backpressure (the TCP default) for unpaced encoders"
                    } else {
                        "pace the encoder in real time"
                    };
                    tracing::warn!(subchannel = %sub.uid, dropped, hint, "input delivers frames faster than real time; dropping frames breaks DAB+ superframes");
                }
            }
            if let Ok(now) = unix_millis_now() {
                let frame_ms = clock.unix_seconds * 1000 + i64::from(clock.millisecond)
                    - i64::from(active.source.ensemble.tist_offset_ms);
                let drift = frame_ms - now;
                stats.clock_drift_ms.store(drift, Ordering::Relaxed);
                if drift.abs() > CLOCK_DRIFT_WARN_MS {
                    if !drift_warned {
                        tracing::warn!(
                            drift_ms = drift,
                            "frame clock has drifted from system time; restart to realign TIST"
                        );
                        drift_warned = true;
                    }
                } else {
                    drift_warned = false;
                }
            }
        }

        seq = seq.wrapping_add(1);
        clock.tick();
        debug_assert_eq!(receivers.len(), active.subchannels.len());
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

    fn sti_packet(fill: u8) -> Vec<u8> {
        AfPacket {
            sequence: u16::from(fill),
            tags: vec![
                pointer_tag(*b"DSTI"),
                dabmux::edi::Tag {
                    name: *b"dsti",
                    value: vec![0, 0],
                },
                dabmux::edi::Tag {
                    name: [b's', b's', 0, 1],
                    value: vec![0, 0, 0, fill, fill, fill],
                },
            ],
        }
        .encode()
        .unwrap()
    }

    async fn recv_bytes(rx: &mut mpsc::Receiver<TimedPayload>) -> Vec<u8> {
        time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("payload in time")
            .expect("open channel")
            .bytes
    }

    #[tokio::test]
    async fn newest_tcp_producer_replaces_a_stale_connection() {
        let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
        let address = listener.local_addr().unwrap();
        let (tx, mut rx) = mpsc::channel(8);
        let stats = Arc::new(RuntimeStats::default());
        let task = tokio::spawn(receive_input_tcp(listener, 1, 3, false, tx, stats));

        let mut stale = TcpStream::connect(address).await.unwrap();
        stale.write_all(&sti_packet(1)).await.unwrap();
        assert_eq!(recv_bytes(&mut rx).await, [1; 3]);

        // The first connection stays open but silent, like a half-open socket
        // after an encoder host reboot. A reconnecting encoder must get through.
        let mut fresh = TcpStream::connect(address).await.unwrap();
        fresh.write_all(&sti_packet(2)).await.unwrap();
        assert_eq!(recv_bytes(&mut rx).await, [2; 3]);

        // The superseded connection is closed on its next frame, which is dropped.
        stale.write_all(&sti_packet(3)).await.unwrap();
        let mut byte = [0u8; 1];
        let closed = time::timeout(Duration::from_secs(2), stale.read(&mut byte))
            .await
            .expect("stale connection closed in time");
        assert!(matches!(closed, Ok(0) | Err(_)));
        fresh.write_all(&sti_packet(4)).await.unwrap();
        assert_eq!(recv_bytes(&mut rx).await, [4; 3]);
        task.abort();
    }

    #[test]
    fn reconfiguration_counter_follows_config_or_structure() {
        assert_eq!(next_reconfiguration_counter(None, None, None, true), None);
        assert_eq!(
            next_reconfiguration_counter(Some(3), Some(3), Some(7), true),
            Some(8)
        );
        assert_eq!(
            next_reconfiguration_counter(Some(3), Some(3), Some(7), false),
            Some(7)
        );
        assert_eq!(
            next_reconfiguration_counter(Some(3), Some(3), Some(1023), true),
            Some(0)
        );
        assert_eq!(
            next_reconfiguration_counter(Some(3), Some(9), Some(7), true),
            Some(9)
        );
        assert_eq!(
            next_reconfiguration_counter(Some(3), None, Some(7), true),
            None
        );
    }

    #[test]
    fn only_multiplex_organisation_counts_as_structure() {
        let base: crate::config::Config =
            serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap();
        let active = base.clone().validate().unwrap();

        let mut label = base.clone();
        label.services[0].label = "Other Label".into();
        label.subchannels[0].input = InputConfig::Edi {
            uri: "udp://127.0.0.1:9100".into(),
            stream_index: 1,
            buffer_frames: 40,
            prebuffer_frames: 4,
            timing: InputTiming::Prebuffering,
            backpressure: None,
        };
        assert_eq!(mci(&label.validate().unwrap()), mci(&active));

        let mut bitrate = base.clone();
        bitrate.subchannels[0].bitrate = 104;
        assert_ne!(mci(&bitrate.validate().unwrap()), mci(&active));

        let mut sid = base;
        sid.services[0].id += 1;
        assert_ne!(mci(&sid.validate().unwrap()), mci(&active));
    }

    #[tokio::test]
    async fn tcp_output_reconfigures_in_place() {
        let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
        let stats = Arc::new(RuntimeStats::default());
        let output = start_tcp_output(0, listener, 10, 5 * 24, stats.clone());
        for n in 0..5u8 {
            output.send(Arc::new(vec![n]), &stats);
        }
        output.reconfigure(20, 2 * 24);
        let state = output.state.lock().unwrap();
        assert_eq!((state.max_queue, state.preroll_frames), (20, 2));
        assert_eq!(
            state.history.iter().map(|p| p[0]).collect::<Vec<_>>(),
            [3, 4]
        );
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
            backpressure: false,
            overflow_drops: 0,
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
    async fn tcp_prebuffering_preserves_frames_when_encoder_runs_ahead() {
        let (tx, rx) = mpsc::channel(8);
        let mut input = BufferedInput {
            rx,
            queue: VecDeque::new(),
            max_frames: 2,
            prebuffer_frames: 1,
            prebuffering: true,
            timing: InputTiming::Prebuffering,
            backpressure: true,
            overflow_drops: 0,
        };
        let stats = RuntimeStats::default();
        let clock = FrameClock::new(0, 0, 0).unwrap();
        for n in 1..=5 {
            tx.send(payload(vec![n], None, None)).await.unwrap();
        }
        for n in 1..=5 {
            assert_eq!(input.take(clock, &stats), Some(vec![n]));
        }
        assert_eq!(stats.input_drops.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn live_input_drops_oldest_when_encoder_runs_ahead() {
        let (tx, rx) = mpsc::channel(8);
        let mut input = BufferedInput {
            rx,
            queue: VecDeque::new(),
            max_frames: 2,
            prebuffer_frames: 1,
            prebuffering: true,
            timing: InputTiming::Prebuffering,
            backpressure: false,
            overflow_drops: 0,
        };
        let stats = RuntimeStats::default();
        let clock = FrameClock::new(0, 0, 0).unwrap();
        for n in 1..=5 {
            tx.send(payload(vec![n], None, None)).await.unwrap();
        }
        assert_eq!(input.take(clock, &stats), Some(vec![4]));
        assert_eq!(input.take(clock, &stats), Some(vec![5]));
        assert_eq!(stats.input_drops.load(Ordering::Relaxed), 3);
        assert_eq!(input.overflow_drops, 3);
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
            backpressure: false,
            overflow_drops: 0,
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
            backpressure: false,
            overflow_drops: 0,
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
