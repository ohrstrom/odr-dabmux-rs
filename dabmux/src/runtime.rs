use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::time::{self, Duration, MissedTickBehavior};

use crate::config::{
    Activation, ConfigUpdate, EdiDestination, InputConfig, InputEndpoint, InputTiming,
    ScheduledSwitch, SharedConfig, Subchannel, SwitchPending, Transport, ValidatedConfig,
};
use crate::fic::{self, FicCarousel};
use crate::timing::{TaiClock, TaiSource};
use dabmux::edi::{
    decode_sti_payload, decode_sti_rtp, fragment_af, pointer_tag, AfPacket, AudioLevels, Deti, Est,
    PftReassembler, TimedPayload,
};
use dabmux::frame::{
    assemble, cifs_per_transmission_frame, mnsc, FrameClock, Stream, FRAME_PERIOD_MS,
};
use dabmux::packet::{LoopingPackets, PacketMultiplexer};

/// Close a TCP producer that sends nothing for this long (C++ uses 10 s too).
const TCP_INPUT_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// Warn when the frame clock and system time disagree by more than this.
const CLOCK_DRIFT_WARN_MS: i64 = 100;
/// Compare the frame clock with system time once per 250 frames (6 s).
const CLOCK_CHECK_FRAMES: u64 = 250;
/// Warn when an input overflows this many frames within one check period.
const DROP_WARN_FRAMES: u64 = 10;
/// After a stall the mux sends the late frames back to back, each with its
/// scheduled frame count and TIST, as ODR-DabMux does: a TIST modulator still
/// plays them. Beyond this lag they are too late for any TIST offset and a
/// burst would overflow the TCP output queues, so the mux skips ahead instead.
const MAX_CATCH_UP: Duration = Duration::from_secs(10);
/// A multiplex reconfiguration switches on a multiple of this many CIFs,
/// which opens a transmission frame in every mode.
const RECONFIGURATION_GRID: u64 = 20;
/// Least announcement of a reconfiguration. Rounded up to the grid, the
/// switch falls 230 to 249 CIFs (5.5 to 6 s) ahead: about the six seconds
/// EN 300 401 clause 6.5 recommends, and within reach of the occurrence change.
const RECONFIGURATION_LEAD: u64 = fic::MAX_ANNOUNCEMENT_FRAMES + 1 - RECONFIGURATION_GRID;
/// A multiplex configuration shall remain stable for at least six seconds
/// (250 CIFs, clause 6.5).
const STABLE_CONFIGURATION_FRAMES: u64 = 250;

/// How to handle a frame whose tick was due `lag` ago.
#[derive(Debug, PartialEq, Eq)]
enum CatchUp {
    OnTime,
    /// Sent at least one frame period late, in a catch-up burst.
    Late,
    /// Skip this many frames to return to the schedule.
    Skip(u64),
}

fn catch_up(lag: Duration) -> CatchUp {
    let period = Duration::from_millis(FRAME_PERIOD_MS);
    if lag > MAX_CATCH_UP {
        CatchUp::Skip((lag.as_micros() / period.as_micros()) as u64)
    } else if lag >= period {
        CatchUp::Late
    } else {
        CatchUp::OnTime
    }
}

pub struct RuntimeStats {
    pub generated_frames: AtomicU64,
    pub config_activations: AtomicU64,
    pub input_underflows: AtomicU64,
    pub input_drops: AtomicU64,
    pub input_size_mismatches: AtomicU64,
    pub decode_errors: AtomicU64,
    pub send_errors: AtomicU64,
    pub missed_ticks: AtomicU64,
    pub catch_up_frames: AtomicU64,
    pub buffered_input_frames: AtomicU64,
    pub late_input_frames: AtomicU64,
    pub invalid_timestamps: AtomicU64,
    pub frame_errors: AtomicU64,
    pub clock_drift_ms: AtomicI64,
    /// The latest frame, for live monitoring; published only while
    /// someone subscribes.
    pub live: watch::Sender<Option<Arc<LiveFrame>>>,
}

impl Default for RuntimeStats {
    fn default() -> Self {
        Self {
            generated_frames: AtomicU64::default(),
            config_activations: AtomicU64::default(),
            input_underflows: AtomicU64::default(),
            input_drops: AtomicU64::default(),
            input_size_mismatches: AtomicU64::default(),
            decode_errors: AtomicU64::default(),
            send_errors: AtomicU64::default(),
            missed_ticks: AtomicU64::default(),
            catch_up_frames: AtomicU64::default(),
            buffered_input_frames: AtomicU64::default(),
            late_input_frames: AtomicU64::default(),
            invalid_timestamps: AtomicU64::default(),
            frame_errors: AtomicU64::default(),
            clock_drift_ms: AtomicI64::default(),
            live: watch::Sender::new(None),
        }
    }
}

/// The state of the mux at one frame.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LiveFrame {
    /// Frames generated since startup.
    pub frame: u64,
    /// The frame's time, in milliseconds since the Unix epoch.
    pub unix_ms: i64,
    pub subchannels: Vec<LiveSubchannel>,
    pub outputs: Vec<LiveOutput>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LiveSubchannel {
    pub name: String,
    pub id: u8,
    pub state: InputState,
    /// Frames waiting in the input buffer; none for a packet file.
    pub buffered: usize,
    pub capacity: usize,
    /// Frames without input data since the input was set up.
    pub underflows: u64,
    /// Frames dropped because the input ran ahead of real time.
    pub drops: u64,
    /// Peak levels the encoder sent with this frame.
    #[serde(skip)]
    pub audio: Option<LiveAudio>,
}

/// Peak levels of one frame, linear 16-bit PCM (0 to 32767).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LiveAudio {
    pub left: i16,
    pub right: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputState {
    Receiving,
    /// Filling the buffer before playing out.
    Prebuffering,
    /// No data; silence or padding is sent instead.
    Underflow,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LiveOutput {
    pub protocol: &'static str,
    /// TCP: the listening port; UDP: the destination.
    pub endpoint: String,
    /// Connected TCP clients.
    pub clients: Option<usize>,
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
            catch_up_frames: self.catch_up_frames.load(Ordering::Relaxed),
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
    /// Frames skipped after falling more than `MAX_CATCH_UP` behind.
    missed_ticks: u64,
    /// Frames sent late, back to back, to catch up after a stall.
    catch_up_frames: u64,
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
    /// Audio levels of the frame played out last, if the encoder sent any.
    levels: Option<AudioLevels>,
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
            InputConfig::Sti { .. } | InputConfig::File { .. } => {
                (64, 1, InputTiming::Prebuffering)
            }
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
            levels: None,
        }
    }

    fn take(&mut self, clock: FrameClock, stats: &RuntimeStats) -> Option<Vec<u8>> {
        let payload = self.take_payload(clock, stats);
        self.levels = payload.as_ref().and_then(|payload| payload.audio_levels);
        payload.map(|payload| payload.bytes)
    }

    fn take_payload(&mut self, clock: FrameClock, stats: &RuntimeStats) -> Option<TimedPayload> {
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
                    Some(payload) => Some(payload),
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
                    return self.queue.pop_front();
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
        InputConfig::File { .. } => unreachable!("file inputs have no socket"),
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
            InputConfig::File { .. } => unreachable!("file inputs have no socket"),
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
    config: Subchannel,
    source: InputSource,
    underflowing: bool,
    reported_drops: u64,
    /// Frames without data, for live monitoring.
    underflows: u64,
}

enum InputSource {
    Network(NetworkInput),
    Packets(PacketInput),
}

struct NetworkInput {
    endpoint: InputEndpoint,
    socket: InputSocket,
    task: tokio::task::JoinHandle<()>,
    buffered: BufferedInput,
}

impl Drop for NetworkInput {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// What an input delivered for one frame.
enum Payload {
    Data(Vec<u8>),
    /// Nothing usable; `substitute` keeps the subchannel valid (silence, or
    /// padding packets with FEC).
    Missing {
        received: Option<usize>,
        substitute: Vec<u8>,
    },
}

impl InputHandle {
    fn endpoint(&self) -> Option<InputEndpoint> {
        match &self.source {
            InputSource::Network(input) => Some(input.endpoint),
            InputSource::Packets(_) => None,
        }
    }

    /// Only the input settings and the frame size matter to a running
    /// receiver; ID, protection and name changes are applied in place.
    fn serves(
        &self,
        endpoint: Option<InputEndpoint>,
        sub: &Subchannel,
        addresses: &BTreeSet<u16>,
    ) -> bool {
        let packets_match = match &self.source {
            InputSource::Packets(input) => input.addresses == *addresses,
            InputSource::Network(_) => true,
        };
        self.endpoint() == endpoint
            && self.config.input == sub.input
            && self.config.bitrate == sub.bitrate
            && packets_match
    }

    fn next_payload(&mut self, clock: FrameClock, stats: &RuntimeStats, bytes: usize) -> Payload {
        match &mut self.source {
            InputSource::Network(input) => match input.buffered.take(clock, stats) {
                Some(data) if data.len() == bytes => Payload::Data(data),
                data => Payload::Missing {
                    received: data.as_ref().map(Vec::len),
                    substitute: vec![0u8; bytes],
                },
            },
            InputSource::Packets(input) => {
                let (frame, had_data) = input.frame(bytes);
                if had_data {
                    Payload::Data(frame)
                } else {
                    Payload::Missing {
                        received: None,
                        substitute: frame,
                    }
                }
            }
        }
    }

    fn network(&self) -> Option<&NetworkInput> {
        match &self.source {
            InputSource::Network(input) => Some(input),
            InputSource::Packets(_) => None,
        }
    }
}

/// Ready-made packets from a file, multiplexed with the clause 5.3.5 FEC.
struct PacketInput {
    /// Packet addresses configured for this subchannel, checked against the file.
    addresses: BTreeSet<u16>,
    source: LoopingPackets,
    mux: PacketMultiplexer,
    updates: watch::Receiver<Option<Arc<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl PacketInput {
    /// One frame, and whether it carried file data rather than padding only.
    fn frame(&mut self, bytes: usize) -> (Vec<u8>, bool) {
        if self.updates.has_changed().unwrap_or(false) {
            let content = self.updates.borrow_and_update().clone();
            self.source.replace(content);
        }
        let had_data = self.source.has_data();
        (self.mux.frame(bytes, &mut self.source), had_data)
    }
}

impl Drop for PacketInput {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// How often a packet input file is checked for changes.
const PACKET_FILE_POLL: Duration = Duration::from_secs(2);

/// Read `path` whenever its size or modification time changes, and publish
/// valid content. An invalid file keeps the previous content; a missing one
/// stops data at the next wrap, as in ODR-DabMux.
async fn watch_packet_file(
    name: String,
    path: std::path::PathBuf,
    addresses: BTreeSet<u16>,
    tx: watch::Sender<Option<Arc<Vec<u8>>>>,
    poll: Duration,
) {
    let mut seen = None;
    let mut missing_warned = false;
    loop {
        match tokio::fs::metadata(&path).await {
            Ok(metadata) => {
                missing_warned = false;
                let stamp = (metadata.len(), metadata.modified().ok());
                if seen != Some(stamp) {
                    seen = Some(stamp);
                    match tokio::fs::read(&path)
                        .await
                        .map_err(anyhow::Error::from)
                        .and_then(|content| Ok((dabmux::packet::scan_packets(&content)?, content)))
                    {
                        Ok((scan, content)) => {
                            let unconfigured: Vec<_> =
                                scan.addresses.difference(&addresses).collect();
                            let absent: Vec<_> = addresses.difference(&scan.addresses).collect();
                            if !unconfigured.is_empty() || !absent.is_empty() {
                                tracing::warn!(subchannel = %name, path = %path.display(), ?unconfigured, ?absent, "packet addresses in the file do not match the configured components");
                            }
                            let message = if tx.borrow().is_some() {
                                "packet file changed; it takes over when the current one wraps"
                            } else {
                                "packet file loaded"
                            };
                            tracing::info!(subchannel = %name, path = %path.display(), packets = scan.packets, bytes = content.len(), "{message}");
                            let _ = tx.send(Some(Arc::new(content)));
                        }
                        Err(err) => {
                            tracing::warn!(subchannel = %name, path = %path.display(), %err, "invalid packet file; keeping the previous content");
                        }
                    }
                }
            }
            Err(err) => {
                if !missing_warned {
                    tracing::warn!(subchannel = %name, path = %path.display(), %err, "packet file unavailable; sending padding after the current content");
                    missing_warned = true;
                }
                if seen.take().is_some() {
                    let _ = tx.send(None);
                }
            }
        }
        time::sleep(poll).await;
    }
}

fn start_packet_input(
    subchannel: Subchannel,
    addresses: BTreeSet<u16>,
    poll: Duration,
) -> InputHandle {
    let InputConfig::File { path } = &subchannel.input else {
        unreachable!("packet inputs read files");
    };
    let (tx, updates) = watch::channel(None);
    let task = tokio::spawn(watch_packet_file(
        subchannel.name.clone(),
        path.clone(),
        addresses.clone(),
        tx,
        poll,
    ));
    InputHandle {
        config: subchannel,
        source: InputSource::Packets(PacketInput {
            addresses,
            source: LoopingPackets::default(),
            mux: PacketMultiplexer::default(),
            updates,
            task,
        }),
        underflowing: false,
        reported_drops: 0,
        underflows: 0,
    }
}

/// Configured packet addresses per subchannel index.
fn packet_addresses(config: &ValidatedConfig, subchannel: usize) -> BTreeSet<u16> {
    config
        .source
        .components
        .iter()
        .filter(|component| component.subchannel == subchannel)
        .filter_map(|component| component.packet.as_ref().map(|packet| packet.address))
        .collect()
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

fn live_frame(
    active: &ValidatedConfig,
    clock: FrameClock,
    receivers: &[InputHandle],
    tcp_outputs: &[TcpOutputHandle],
    udp_destinations: &[String],
    frame: u64,
) -> LiveFrame {
    let subchannels = active
        .subchannels
        .iter()
        .zip(receivers)
        .map(|(sub, input)| {
            let (buffered, capacity, prebuffering, drops, levels) = match &input.source {
                InputSource::Network(network) => (
                    network.buffered.queue.len(),
                    network.buffered.max_frames,
                    network.buffered.prebuffering,
                    network.buffered.overflow_drops,
                    network.buffered.levels,
                ),
                InputSource::Packets(_) => (0, 0, false, 0, None),
            };
            let state = if input.underflowing {
                InputState::Underflow
            } else if prebuffering {
                InputState::Prebuffering
            } else {
                InputState::Receiving
            };
            LiveSubchannel {
                name: sub.name.clone(),
                id: sub.id,
                state,
                buffered,
                capacity,
                underflows: input.underflows,
                drops,
                audio: levels.map(|levels| LiveAudio {
                    left: levels.left,
                    right: levels.right,
                }),
            }
        })
        .collect();
    let outputs = tcp_outputs
        .iter()
        .map(|output| LiveOutput {
            protocol: "tcp",
            endpoint: output.port.to_string(),
            clients: Some(
                output
                    .state
                    .lock()
                    .expect("TCP output state poisoned")
                    .clients
                    .len(),
            ),
        })
        .chain(udp_destinations.iter().map(|destination| LiveOutput {
            protocol: "udp",
            endpoint: destination.clone(),
            clients: None,
        }))
        .collect();
    LiveFrame {
        frame,
        unix_ms: clock.unix_seconds * 1000 + i64::from(clock.millisecond),
        subchannels,
        outputs,
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

/// How long the loopback probe of [`refuse_shadowed_port`] waits.
const LOOPBACK_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// Refuse a wildcard TCP bind that another listener on 127.0.0.1 would shadow.
/// Tokio binds with SO_REUSEADDR, and on macOS and the BSDs that lets
/// `0.0.0.0:port` succeed while another process listens on
/// `127.0.0.1:port`; local clients then reach that process instead of the
/// mux. Linux rejects such a bind itself, so the probe only finds nothing.
async fn refuse_shadowed_port(
    address: std::net::SocketAddr,
    what: &str,
    held_ports: &HashSet<u16>,
) -> Result<()> {
    if !address.ip().is_unspecified() {
        return Ok(());
    }
    let port = address.port();
    let loopback = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    if let Ok(Ok(_)) = time::timeout(LOOPBACK_PROBE_TIMEOUT, TcpStream::connect(loopback)).await {
        if held_ports.contains(&port) {
            bail!(
                "binding {what}: port {port} is still held by the active configuration; \
                 release it in one update and reuse it in a later one"
            );
        }
        bail!(
            "binding {what}: another process already listens on {loopback}, so local \
             clients would reach it instead of the mux; stop it or choose another port"
        );
    }
    Ok(())
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
        let Some(endpoint) = sub.endpoint else {
            continue;
        };
        if existing_inputs.contains(&endpoint) || prepared.inputs.contains_key(&endpoint) {
            continue;
        }
        let port = endpoint.address.port();
        if endpoint.transport == Transport::Tcp {
            refuse_shadowed_port(endpoint.address, &format!("input {endpoint}"), &held_ports)
                .await?;
        }
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
            let address = std::net::SocketAddr::from(([0, 0, 0, 0], *listen_port));
            refuse_shadowed_port(address, "EDI TCP output", &held_ports).await?;
            let listener = TcpListener::bind(address)
                .await
                .map_err(|err| bind_error(err, "EDI TCP output", *listen_port, &held_ports))?;
            prepared.outputs.insert(*listen_port, Arc::new(listener));
        }
    }
    Ok(prepared)
}

fn start_input(
    subchannel: Subchannel,
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
        InputConfig::Sti { .. } | InputConfig::File { .. } => 64,
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
        config: subchannel,
        source: InputSource::Network(NetworkInput {
            endpoint,
            socket,
            task,
            buffered,
        }),
        underflowing: false,
        reported_drops: 0,
        underflows: 0,
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
    for (index, (sub, validated)) in candidate
        .source
        .subchannels
        .iter()
        .zip(&candidate.subchannels)
        .enumerate()
    {
        let endpoint = validated.endpoint;
        let addresses = packet_addresses(candidate, index);
        if let Some(position) = inputs
            .iter()
            .position(|old| old.serves(endpoint, sub, &addresses))
        {
            let mut handle = inputs.swap_remove(position);
            handle.config = sub.clone();
            next_inputs.push(handle);
            continue;
        }
        let Some(endpoint) = endpoint else {
            next_inputs.push(start_packet_input(sub.clone(), addresses, PACKET_FILE_POLL));
            continue;
        };
        let socket = if let Some(position) = inputs
            .iter()
            .position(|old| old.endpoint() == Some(endpoint))
        {
            let old = inputs.swap_remove(position);
            let socket = old.network().expect("network input").socket.clone();
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

type Reply = tokio::sync::oneshot::Sender<Result<Activation>>;

/// A multiplex reconfiguration announced in the FIC.
struct Announced {
    candidate: ValidatedConfig,
    resources: PreparedResources,
    /// Frame count of the first frame of `candidate`.
    at: u64,
    switch: ScheduledSwitch,
}

/// The system time and CIF count of frame `at`, seen from `clock`.
fn scheduled_switch(clock: FrameClock, active: &ValidatedConfig, at: u64) -> ScheduledSwitch {
    // The frame clock runs ahead of system time by the TIST offset.
    let millis = clock.unix_seconds * 1000 + i64::from(clock.millisecond)
        - i64::from(active.source.ensemble.tist_offset_ms)
        + ((at - clock.count) * FRAME_PERIOD_MS) as i64;
    ScheduledSwitch {
        at: chrono::DateTime::from_timestamp_millis(millis).unwrap_or_default(),
        cif_count: (at % 5000) as u16,
    }
}

/// The frame from which an announced reconfiguration applies: on the grid,
/// at least the lead ahead of `count`, and no sooner than the active
/// configuration, in force since `since`, may change.
fn reconfiguration_frame(count: u64, since: u64) -> u64 {
    (count + RECONFIGURATION_LEAD)
        .max(since + STABLE_CONFIGURATION_FRAMES)
        .next_multiple_of(RECONFIGURATION_GRID)
}

/// The frame clock once `candidate` replaces `active`, which may shift TIST
/// or rephase the frame count.
fn clock_for(
    clock: FrameClock,
    active: &ValidatedConfig,
    candidate: &ValidatedConfig,
) -> Result<FrameClock> {
    let mut next = clock;
    next.shift_millis(
        i64::from(candidate.source.ensemble.tist_offset_ms)
            - i64::from(active.source.ensemble.tist_offset_ms),
    )?;
    if candidate.source.ensemble.tist_at_fct0_ms != active.source.ensemble.tist_at_fct0_ms {
        next.rephase_fct0(candidate.source.ensemble.tist_at_fct0_ms)?;
    }
    Ok(next)
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
    tick.set_missed_tick_behavior(MissedTickBehavior::Burst);
    for warning in initial.warnings() {
        tracing::warn!("{warning}");
    }
    let mut active = initial;
    let mut pending: Option<(ConfigUpdate, PreparedResources)> = None;
    // A reconfiguration announced in the FIC, and the frame it applies from.
    let mut announced: Option<Announced> = None;
    // Frame from which the active multiplex configuration applies.
    let mut configuration_since = clock.count;
    let mut frame_failing = false;
    let mut drift_warned = false;

    loop {
        let instant = tokio::select! {
            instant = tick.tick() => instant,
            Some(update) = updates.recv() => {
                let input_endpoints: HashSet<_> = receivers.iter().filter_map(InputHandle::endpoint).collect();
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
        match catch_up(time::Instant::now().saturating_duration_since(instant)) {
            CatchUp::OnTime => {}
            CatchUp::Late => {
                stats.catch_up_frames.fetch_add(1, Ordering::Relaxed);
            }
            CatchUp::Skip(frames) => {
                // Keep the 24 ms grid: this frame takes the slot `frames` later.
                stats.missed_ticks.fetch_add(frames, Ordering::Relaxed);
                for _ in 0..frames {
                    clock.tick();
                }
                let period = Duration::from_millis(FRAME_PERIOD_MS);
                tick.reset_at(instant + period * (frames as u32 + 1));
                tracing::warn!(
                    skipped_frames = frames,
                    "mux fell more than {} s behind; skipping ahead instead of catching up",
                    MAX_CATCH_UP.as_secs()
                );
            }
        }

        // Switch configurations only where a transmission frame begins. A
        // multiplex reconfiguration is announced first and switches on the
        // announced frame; other changes switch right away.
        let frame_boundary = clock
            .count
            .is_multiple_of(cifs_per_transmission_frame(active.source.ensemble.mode));
        // The reply goes out when the change is active, or for a
        // reconfiguration once it is scheduled.
        let mut due: Option<(ValidatedConfig, PreparedResources, Option<Reply>)> = announced
            .take_if(|next| frame_boundary && clock.count >= next.at)
            .map(|next| (next.candidate, next.resources, None));
        if let Some(next) = &announced {
            // SharedConfig refuses changes until the switch; this is a backstop.
            if let Some((update, _)) = pending.take() {
                let _ = update
                    .reply
                    .send(Err(SwitchPending(next.switch.clone()).into()));
            }
        }
        if let Some((update, resources)) = pending.take_if(|_| frame_boundary) {
            let ConfigUpdate { candidate, reply } = update;
            if !fic::reconfigures(&active, &candidate) {
                due = Some((candidate, resources, Some(reply)));
            } else if let Err(err) = clock_for(clock, &active, &candidate) {
                let _ = reply.send(Err(err));
            } else {
                let at = reconfiguration_frame(clock.count, configuration_since);
                let counter = next_reconfiguration_counter(
                    active.source.ensemble.reconfiguration_counter,
                    candidate.source.ensemble.reconfiguration_counter,
                    reconfiguration_counter,
                    true,
                );
                let switch = scheduled_switch(clock, &active, at);
                carousel.announce(&candidate, at, counter);
                config.schedule(switch.clone()).await;
                tracing::info!(
                    at = %switch.at,
                    cif_count = switch.cif_count,
                    "multiplex reconfiguration announced"
                );
                let _ = reply.send(Ok(Activation::Scheduled(switch.clone())));
                announced = Some(Announced {
                    candidate,
                    resources,
                    at,
                    switch,
                });
            }
        }
        if let Some((candidate, resources, reply)) = due {
            match clock_for(clock, &active, &candidate) {
                Err(err) => {
                    carousel.withdraw();
                    match reply {
                        Some(reply) => {
                            let _ = reply.send(Err(err));
                        }
                        None => {
                            config.unschedule().await;
                            tracing::error!(%err, "announced multiplex reconfiguration failed; the previous configuration stays active");
                        }
                    }
                }
                Ok(next_clock) => {
                    let structure_changed = fic::reconfigures(&active, &candidate);
                    clock = next_clock;
                    for warning in candidate.warnings() {
                        tracing::warn!("{warning}");
                    }
                    let reallocated = candidate.reallocated_subchannels(&active);
                    if !reallocated.is_empty() {
                        let changes = reallocated
                            .iter()
                            .map(|(name, from, to)| format!("{name} {from}->{to}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        tracing::warn!(
                            %changes,
                            "allocated SubChIds changed; receivers lose these services until they rescan; set subchannel_id to keep them stable"
                        );
                    }
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
                        configuration_since = clock.count;
                        for output in &tcp_outputs {
                            output.clear_history();
                        }
                        tracing::info!(
                            reconfiguration_counter,
                            "multiplex reconfiguration applied; TCP preroll cleared"
                        );
                    }
                    let new_tai_source = TaiSource::from_config(&candidate.source.ensemble);
                    if tai_clock.source != new_tai_source {
                        tai_clock = TaiClock::new(new_tai_source, tai_offset);
                    }
                    let events = fic::DatabaseEvents::between(&active, &candidate);
                    if !events.is_empty() {
                        tracing::info!("service following databases changed; signalling change indications for 5 s");
                    }
                    active = candidate.clone();
                    carousel = FicCarousel::new()
                        .with_reconfiguration_counter(reconfiguration_counter)
                        .with_database_events(events, clock.count);
                    config.commit(candidate).await;
                    stats.config_activations.fetch_add(1, Ordering::Relaxed);
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(Activation::Applied));
                    }
                    tracing::info!("complete mux configuration activated");
                }
            }
        }

        let mut payloads = Vec::with_capacity(active.subchannels.len());
        for (sub, input) in active.subchannels.iter().zip(&mut receivers) {
            let data = match input.next_payload(clock, &stats, sub.payload_bytes) {
                Payload::Data(data) => {
                    if input.underflowing {
                        tracing::info!(subchannel = %sub.name, "input recovered");
                        input.underflowing = false;
                    }
                    data
                }
                Payload::Missing {
                    received,
                    substitute,
                } => {
                    stats.input_underflows.fetch_add(1, Ordering::Relaxed);
                    input.underflows += 1;
                    if !input.underflowing {
                        let substitute_kind = match input.source {
                            InputSource::Network(_) => "silence",
                            InputSource::Packets(_) => "padding packets",
                        };
                        tracing::warn!(subchannel = %sub.name, received_bytes = ?received, expected_bytes = sub.payload_bytes, substitute = substitute_kind, "input underflow");
                        input.underflowing = true;
                    }
                    substitute
                }
            };
            payloads.push(data);
        }
        if stats.live.receiver_count() > 0 {
            let frame = live_frame(
                &active,
                clock,
                &receivers,
                &tcp_outputs,
                &udp_destinations,
                stats.generated_frames.load(Ordering::Relaxed),
            );
            stats.live.send_replace(Some(Arc::new(frame)));
        }
        stats.buffered_input_frames.store(
            receivers
                .iter()
                .filter_map(InputHandle::network)
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
                let Some(network) = input.network() else {
                    continue;
                };
                let (overflow_drops, transport) =
                    (network.buffered.overflow_drops, network.endpoint.transport);
                let dropped = overflow_drops - input.reported_drops;
                input.reported_drops = overflow_drops;
                if dropped >= DROP_WARN_FRAMES {
                    let hint = if transport == Transport::Tcp {
                        "enable backpressure (the TCP default) for unpaced encoders"
                    } else {
                        "pace the encoder in real time"
                    };
                    tracing::warn!(subchannel = %sub.name, dropped, hint, "input delivers frames faster than real time; dropping frames breaks DAB+ superframes");
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
    use crate::config::SubchannelKind;

    fn payload(bytes: Vec<u8>, seconds: Option<u32>, tsta: Option<u32>) -> TimedPayload {
        TimedPayload {
            dlfc: 0,
            utco: seconds.map(|_| 0),
            seconds,
            tsta,
            stream_index: 1,
            bytes,
            audio_levels: None,
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
        let base = crate::config::testing::example();
        let active = base.clone().validate().unwrap();

        let mut label = base.clone();
        label.services[0].label = "Other Label".into();
        label.services[0].components[0].input = Some(crate::config::schema::InputSpec::Edi {
            uri: "udp://127.0.0.1:9100".into(),
            stream_index: None,
            buffer_frames: Some(80),
            prebuffer_frames: None,
            timing: None,
            backpressure: None,
        });
        assert!(!fic::reconfigures(&active, &label.validate().unwrap()));

        let mut bitrate = base.clone();
        bitrate.services[0].components[0].bitrate = Some(104);
        assert!(fic::reconfigures(&active, &bitrate.validate().unwrap()));

        let mut sid = base.clone();
        sid.services[0].id += 1;
        assert!(fic::reconfigures(&active, &sid.validate().unwrap()));

        // FIG 0/13 is MCI too.
        let mut application = base;
        application.services[0].components[0].user_applications =
            vec![crate::config::UserApplication::Slideshow];
        assert!(fic::reconfigures(&active, &application.validate().unwrap()));
    }

    #[test]
    fn reconfiguration_switches_on_the_grid_about_six_seconds_ahead() {
        // Long stable: 230 to 249 CIFs ahead, so the occurrence change is
        // unambiguous, and on a frame that opens a transmission frame.
        for count in 1000..1100 {
            let at = reconfiguration_frame(count, 0);
            assert!((230..=249).contains(&(at - count)), "{count} -> {at}");
            assert!(at.is_multiple_of(20));
        }
        // Six seconds after the last switch at the earliest.
        assert_eq!(reconfiguration_frame(1000, 980), 1240);
        assert_eq!(reconfiguration_frame(1000, 1000), 1260);
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
            levels: None,
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
    async fn audio_levels_follow_the_frame_played_out() {
        let (tx, rx) = mpsc::channel(8);
        let mut input = BufferedInput::new(
            rx,
            &InputConfig::Sti {
                uri: "rtp://:9000".into(),
            },
        );
        let stats = RuntimeStats::default();
        let clock = FrameClock::new(0, 0, 0).unwrap();
        for (byte, left) in [(1, 100), (2, 200)] {
            let mut frame = payload(vec![byte], None, None);
            frame.audio_levels = Some(AudioLevels { left, right: 0 });
            tx.send(frame).await.unwrap();
        }
        assert_eq!(input.take(clock, &stats), Some(vec![1]));
        assert_eq!(input.levels.map(|l| l.left), Some(100));
        assert_eq!(input.take(clock, &stats), Some(vec![2]));
        assert_eq!(input.levels.map(|l| l.left), Some(200));
        assert_eq!(input.take(clock, &stats), None);
        assert_eq!(input.levels, None, "no levels without audio on air");
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
            levels: None,
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
            levels: None,
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
            levels: None,
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
            levels: None,
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

    #[tokio::test]
    async fn packet_file_input_loads_switches_at_the_wrap_and_pads_when_missing() {
        use dabmux::packet::{packet_address, verify_fec_stream, FEC_ADDRESS, PADDING_ADDRESS};
        fn packet(address: u16) -> Vec<u8> {
            let mut packet = vec![0u8; 24];
            packet[0] = 0x30 | (address >> 8) as u8;
            packet[1] = address as u8;
            let crc = dabmux::edi::crc16(&packet[..22]);
            packet[22..].copy_from_slice(&crc.to_be_bytes());
            packet
        }
        let dir = std::env::temp_dir().join(format!("dabmux-packet-input-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("spi.bin");
        std::fs::write(&path, [packet(1), packet(1)].concat()).unwrap();
        let subchannel = Subchannel {
            name: "spi".into(),
            id: 30,
            allocated: false,
            kind: SubchannelKind::EnhancedPacket,
            bitrate: 8,
            protection: crate::config::ProtectionConfig::EepA { level: 3 },
            input: InputConfig::File { path: path.clone() },
        };
        let mut input =
            start_packet_input(subchannel, BTreeSet::from([1]), Duration::from_millis(10));
        let stats = RuntimeStats::default();
        let clock = FrameClock::new(0, 0, 0).unwrap();
        let next = |input: &mut InputHandle| match input.next_payload(clock, &stats, 24) {
            Payload::Data(data) => (true, data),
            Payload::Missing { substitute, .. } => (false, substitute),
        };
        let mut stream = Vec::new();
        let (had_data, frame) = next(&mut input);
        assert!(
            !had_data && packet_address(&frame) == PADDING_ADDRESS,
            "padding before the load"
        );
        stream.extend(frame);
        time::sleep(Duration::from_millis(100)).await;
        for _ in 0..2 {
            let (had_data, frame) = next(&mut input);
            assert!(had_data && packet_address(&frame) == 1);
            stream.extend(frame);
        }

        // The old file has just wrapped, so a new one takes over at once;
        // mid-file it would wait for the wrap.
        std::fs::write(&path, [packet(2), packet(2), packet(2)].concat()).unwrap();
        time::sleep(Duration::from_millis(100)).await;
        let (_, frame) = next(&mut input);
        assert_eq!(packet_address(&frame), 2);
        stream.extend(frame);

        // Without the file, data stops at the next wrap; FEC continues.
        std::fs::remove_file(&path).unwrap();
        time::sleep(Duration::from_millis(100)).await;
        let mut addresses = Vec::new();
        for _ in 0..210 {
            let (_, frame) = next(&mut input);
            addresses.push(packet_address(&frame));
            stream.extend(frame);
        }
        assert_eq!(&addresses[..2], &[2, 2], "the rest of the current pass");
        assert!(addresses[2..]
            .iter()
            .all(|&a| a == PADDING_ADDRESS || a == FEC_ADDRESS));
        assert!(matches!(next(&mut input), (false, _)));
        assert_eq!(verify_fec_stream(&stream), Ok(2));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn late_frames_are_caught_up_in_a_burst_up_to_ten_seconds() {
        let ms = Duration::from_millis;
        assert_eq!(catch_up(ms(0)), CatchUp::OnTime);
        assert_eq!(catch_up(ms(23)), CatchUp::OnTime);
        assert_eq!(catch_up(ms(24)), CatchUp::Late);
        assert_eq!(catch_up(ms(10_000)), CatchUp::Late);
        // 10.008 s behind: 417 whole frames to skip.
        assert_eq!(catch_up(ms(10_008)), CatchUp::Skip(417));
    }

    #[tokio::test(start_paused = true)]
    async fn burst_ticks_keep_their_scheduled_instants() {
        let period = Duration::from_millis(FRAME_PERIOD_MS);
        let mut tick = time::interval(period);
        tick.set_missed_tick_behavior(MissedTickBehavior::Burst);
        let start = tick.tick().await;
        time::advance(Duration::from_millis(100)).await;
        // Four ticks were due during the stall; they come at once, on schedule.
        for n in 1..=4u32 {
            let instant = tick.tick().await;
            assert_eq!(instant, start + period * n);
            assert_eq!(
                catch_up(time::Instant::now() - instant),
                if n < 4 {
                    CatchUp::Late
                } else {
                    CatchUp::OnTime
                }
            );
        }
    }

    #[tokio::test]
    async fn wildcard_bind_refuses_a_port_shadowed_on_loopback() {
        // Another "process": a listener on 127.0.0.1 only.
        let other = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = other.local_addr().unwrap().port();
        let wildcard = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        let err = refuse_shadowed_port(wildcard, "EDI TCP output", &HashSet::new())
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("another process already listens on 127.0.0.1:"),
            "{err}"
        );
        // Our own active configuration holding the port gets the reload hint.
        let err = refuse_shadowed_port(wildcard, "EDI TCP output", &HashSet::from([port]))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("still held by the active configuration"),
            "{err}"
        );
        // Specific addresses are left to bind() itself; free ports pass.
        let specific = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        assert!(refuse_shadowed_port(specific, "input", &HashSet::new())
            .await
            .is_ok());
        drop(other);
        assert!(
            refuse_shadowed_port(wildcard, "EDI TCP output", &HashSet::new())
                .await
                .is_ok()
        );
    }
}
