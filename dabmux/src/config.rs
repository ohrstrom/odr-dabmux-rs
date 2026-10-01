use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

mod number;
pub mod reload;
pub mod schema;
pub mod watch;

pub use schema::Config;

pub struct ConfigUpdate {
    pub candidate: ValidatedConfig,
    pub reply: oneshot::Sender<anyhow::Result<bool>>,
}

#[derive(Clone)]
pub struct SharedConfig {
    active: Arc<RwLock<ValidatedConfig>>,
    updates: Arc<Mutex<Option<mpsc::Sender<ConfigUpdate>>>>,
    update_lock: Arc<Mutex<()>>,
}

impl SharedConfig {
    pub fn new(config: ValidatedConfig) -> Self {
        Self {
            active: Arc::new(RwLock::new(config)),
            updates: Arc::new(Mutex::new(None)),
            update_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, ValidatedConfig> {
        self.active.read().await
    }

    pub async fn install_updates(&self, sender: mpsc::Sender<ConfigUpdate>) {
        *self.updates.lock().await = Some(sender);
    }

    pub async fn commit(&self, config: ValidatedConfig) {
        *self.active.write().await = config;
    }

    pub async fn replace_if_changed(&self, config: ValidatedConfig) -> anyhow::Result<bool> {
        let _guard = self.update_lock.lock().await;
        if *self.active.read().await == config {
            return Ok(false);
        }
        let sender = self
            .updates
            .lock()
            .await
            .clone()
            .context("mux runtime is not ready for config updates")?;
        let (reply, result) = oneshot::channel();
        sender
            .send(ConfigUpdate {
                candidate: config,
                reply,
            })
            .await
            .context("mux runtime stopped accepting config updates")?;
        result
            .await
            .context("mux runtime stopped during config update")?
    }

    pub async fn apply(&self, candidate: Config) -> anyhow::Result<bool> {
        self.replace_if_changed(candidate.validate()?).await
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnsembleConfig {
    #[serde(deserialize_with = "number::deserialize")]
    pub id: u16,
    #[serde(deserialize_with = "number::deserialize")]
    pub ecc: u8,
    pub label: String,
    pub short_label: Option<String>,
    #[serde(default)]
    pub local_time_offset_half_hours: i8,
    #[serde(default)]
    pub local_time_offset_auto: bool,
    #[serde(default)]
    pub international_table: u8,
    pub reconfiguration_counter: Option<u16>,
    #[serde(default = "default_mode")]
    pub mode: u8,
    #[serde(default)]
    pub tist: bool,
    #[serde(default)]
    pub tist_offset_ms: i32,
    #[serde(default)]
    pub tist_at_fct0_ms: u16,
    pub tai_utc_offset: Option<u8>,
    /// HTTPS leap-second bulletin URLs, tried in order.
    #[serde(default)]
    pub tai_clock_bulletins: Vec<String>,
}

fn default_mode() -> u8 {
    1
}

/// The flat multiplex model produced by [`Config::normalize`]. Components and
/// subchannels refer to each other by index; subchannels are in MSC order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Multiplex {
    pub ensemble: EnsembleConfig,
    pub services: Vec<Service>,
    pub subchannels: Vec<Subchannel>,
    /// Grouped by service, in component order within each service.
    pub components: Vec<Component>,
    pub output: EdiOutputConfig,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Service {
    pub id: u32,
    pub label: String,
    pub short_label: Option<String>,
    pub pty: u8,
    pub language: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    /// Index into `services`.
    pub service: usize,
    /// Index into `subchannels`.
    pub subchannel: usize,
    pub user_applications: Vec<UserApplication>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subchannel {
    /// Shared subchannel name, or `<SId>/<component position>`; used in logs.
    pub name: String,
    /// Transmitted SubChId.
    pub id: u8,
    /// Whether `id` was allocated rather than configured.
    pub allocated: bool,
    pub kind: SubchannelKind,
    pub bitrate: u16,
    pub protection: ProtectionConfig,
    pub input: InputConfig,
}

/// Display form of a service ID: four hex digits for programme services,
/// eight for 32-bit data service IDs.
pub fn service_name(id: u32) -> String {
    if id <= u32::from(u16::MAX) {
        format!("{id:04X}")
    } else {
        format!("{id:08X}")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserApplication {
    Slideshow,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubchannelKind {
    DabPlus,
    MpegAudio,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "profile", deny_unknown_fields)]
pub enum ProtectionConfig {
    EepA { level: u8 },
    EepB { level: u8 },
}

/// A subchannel input with all defaults resolved.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "protocol")]
pub enum InputConfig {
    Edi {
        uri: String,
        stream_index: u16,
        buffer_frames: usize,
        prebuffer_frames: usize,
        timing: InputTiming,
        /// `None` selects the transport default; see [`InputConfig::backpressure`].
        backpressure: Option<bool>,
    },
    Sti {
        uri: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    Udp,
    Tcp,
}

/// Local socket an input listens on, resolved once from its URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputEndpoint {
    pub transport: Transport,
    pub address: std::net::SocketAddr,
}

impl std::fmt::Display for InputEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = match self.transport {
            Transport::Udp => "udp",
            Transport::Tcp => "tcp",
        };
        write!(f, "{scheme}://{}", self.address)
    }
}

impl InputConfig {
    pub fn uri(&self) -> &str {
        match self {
            Self::Edi { uri, .. } | Self::Sti { uri } => uri,
        }
    }

    pub fn endpoint(&self) -> anyhow::Result<InputEndpoint> {
        let uri = self.uri();
        let (transport, address) = match self {
            Self::Edi { .. } => match (uri.strip_prefix("udp://"), uri.strip_prefix("tcp://")) {
                (Some(address), _) => (Transport::Udp, address),
                (_, Some(address)) => (Transport::Tcp, address),
                _ => bail!("EDI input URI must start with udp:// or tcp://"),
            },
            Self::Sti { .. } => (
                Transport::Udp,
                uri.strip_prefix("rtp://")
                    .context("STI input URI must start with rtp://")?,
            ),
        };
        let address = if address.starts_with(':') {
            format!("0.0.0.0{address}")
        } else {
            address.to_owned()
        };
        let address = address
            .parse()
            .context("input URI needs an IP address and port")?;
        Ok(InputEndpoint { transport, address })
    }

    /// Whether a full buffer should stall the producer instead of dropping.
    /// Defaults to on for TCP; UDP and STI inputs cannot be throttled.
    pub fn backpressure(&self) -> bool {
        match self {
            Self::Edi { backpressure, .. } => backpressure.unwrap_or_else(|| {
                self.endpoint()
                    .is_ok_and(|endpoint| endpoint.transport == Transport::Tcp)
            }),
            Self::Sti { .. } => false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputTiming {
    #[default]
    Prebuffering,
    Timestamped,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EdiOutputConfig {
    pub destinations: Vec<EdiDestination>,
    #[serde(default = "default_tagpacket_alignment")]
    pub tagpacket_alignment: u8,
}

fn default_tagpacket_alignment() -> u8 {
    8
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "protocol", deny_unknown_fields)]
pub enum EdiDestination {
    Udp {
        address: String,
        port: u16,
    },
    Tcp {
        listen_port: u16,
        #[serde(default = "default_tcp_queue")]
        max_frames_queued: usize,
        #[serde(default)]
        preroll_ms: u32,
    },
}

fn default_tcp_queue() -> usize {
    500
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedConfig {
    pub source: Multiplex,
    pub subchannels: Vec<ValidatedSubchannel>,
    /// Components in `source.components` order, with their SCIdS.
    pub components: Vec<ValidatedComponent>,
    pub fic_words: usize,
    pub frame_words: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSubchannel {
    pub name: String,
    pub id: u8,
    pub bitrate: u16,
    pub start_address_cu: u16,
    pub size_cu: u16,
    pub payload_bytes: usize,
    pub tpl: u8,
    pub endpoint: InputEndpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedComponent {
    /// Index into `source.services`.
    pub service: usize,
    /// Index into `subchannels` and `source.subchannels`.
    pub subchannel: usize,
    /// Position within the service; 0 is the primary component.
    pub scids: u8,
}

impl ValidatedConfig {
    pub fn service_components(&self, service: usize) -> impl Iterator<Item = &ValidatedComponent> {
        self.components.iter().filter(move |c| c.service == service)
    }

    /// Subchannels kept across an update whose allocated SubChId changed,
    /// as `(name, previous, new)`. Receivers lose such a service until they
    /// rescan, which a configured `subchannel_id` avoids.
    pub fn reallocated_subchannels(&self, previous: &ValidatedConfig) -> Vec<(String, u8, u8)> {
        self.source
            .subchannels
            .iter()
            .filter(|sub| sub.allocated)
            .filter_map(|sub| {
                let old = previous
                    .source
                    .subchannels
                    .iter()
                    .find(|old| old.allocated && old.name == sub.name)?;
                (old.id != sub.id).then(|| (sub.name.clone(), old.id, sub.id))
            })
            .collect()
    }

    /// The configuration as the mux runs it: allocated SubChIds, CU layout
    /// and all defaults filled in.
    pub fn resolved(&self) -> ResolvedConfig<'_> {
        let source = &self.source;
        ResolvedConfig {
            ensemble: &source.ensemble,
            services: source
                .services
                .iter()
                .enumerate()
                .map(|(index, service)| ResolvedService {
                    service,
                    components: self
                        .components
                        .iter()
                        .zip(&source.components)
                        .filter(|(component, _)| component.service == index)
                        .map(|(component, raw)| ResolvedComponent {
                            scids: component.scids,
                            subchannel: &self.subchannels[component.subchannel].name,
                            user_applications: &raw.user_applications,
                        })
                        .collect(),
                })
                .collect(),
            subchannels: self
                .subchannels
                .iter()
                .zip(&source.subchannels)
                .map(|(sub, raw)| ResolvedSubchannel {
                    name: &sub.name,
                    id: sub.id,
                    allocated: raw.allocated,
                    kind: &raw.kind,
                    bitrate: sub.bitrate,
                    protection: &raw.protection,
                    start_address_cu: sub.start_address_cu,
                    size_cu: sub.size_cu,
                    input: &raw.input,
                })
                .collect(),
            output: &source.output,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ResolvedConfig<'a> {
    pub ensemble: &'a EnsembleConfig,
    pub services: Vec<ResolvedService<'a>>,
    pub subchannels: Vec<ResolvedSubchannel<'a>>,
    pub output: &'a EdiOutputConfig,
}

#[derive(Debug, Serialize)]
pub struct ResolvedService<'a> {
    #[serde(flatten)]
    pub service: &'a Service,
    pub components: Vec<ResolvedComponent<'a>>,
}

#[derive(Debug, Serialize)]
pub struct ResolvedComponent<'a> {
    pub scids: u8,
    pub subchannel: &'a str,
    pub user_applications: &'a [UserApplication],
}

#[derive(Debug, Serialize)]
pub struct ResolvedSubchannel<'a> {
    pub name: &'a str,
    pub id: u8,
    pub allocated: bool,
    #[serde(rename = "type")]
    pub kind: &'a SubchannelKind,
    pub bitrate: u16,
    pub protection: &'a ProtectionConfig,
    pub start_address_cu: u16,
    pub size_cu: u16,
    pub input: &'a InputConfig,
}

impl Multiplex {
    pub fn validate(self) -> anyhow::Result<ValidatedConfig> {
        let mode = self.ensemble.mode;
        if !(1..=4).contains(&mode) {
            bail!("ensemble.mode must be 1..=4");
        }
        if !(-24..=24).contains(&self.ensemble.local_time_offset_half_hours) {
            bail!("ensemble.local_time_offset_half_hours must be -24..=24");
        }
        if self.ensemble.local_time_offset_auto && self.ensemble.local_time_offset_half_hours != 0 {
            bail!("set either automatic or fixed local time offset");
        }
        if self.ensemble.tist_offset_ms.unsigned_abs() > 60_000 {
            bail!("ensemble.tist_offset_ms must be within one minute");
        }
        if self.ensemble.tist_at_fct0_ms >= 1000 {
            bail!("ensemble.tist_at_fct0_ms must be below 1000");
        }
        if let Some(counter) = self.ensemble.reconfiguration_counter {
            if counter > 1023 || self.services.len() > 63 {
                bail!("invalid reconfiguration counter or service count");
            }
        }
        if self.ensemble.tist
            && !matches!(self.ensemble.tai_utc_offset, Some(32..=255))
            && self.ensemble.tai_clock_bulletins.is_empty()
        {
            bail!("ensemble.tist requires tai_utc_offset or tai_clock_bulletins");
        }
        if self
            .ensemble
            .tai_utc_offset
            .is_some_and(|offset| offset < 32)
        {
            bail!("ensemble.tai_utc_offset must be at least 32");
        }
        if self
            .ensemble
            .tai_clock_bulletins
            .iter()
            .any(|url| !url.starts_with("https://") || url.len() < 10)
        {
            bail!("tai_clock_bulletins must be a list of HTTPS URLs");
        }
        validate_label("ensemble.label", &self.ensemble.label)?;
        short_label_mask(
            "ensemble.short_label",
            &self.ensemble.label,
            self.ensemble.short_label.as_deref(),
        )?;
        if self.services.is_empty() {
            bail!("at least one service is required");
        }
        if self.subchannels.len() > 64 {
            bail!("at most 64 subchannels are supported");
        }
        if self.output.destinations.is_empty() {
            bail!("at least one EDI destination is required");
        }
        if !matches!(self.output.tagpacket_alignment, 8 | 16) {
            bail!("output.tagpacket_alignment must be 8 or 16");
        }

        let mut service_ids = HashSet::new();
        for service in &self.services {
            let name = service_name(service.id);
            validate_label(&format!("service {name} label"), &service.label)?;
            if service.pty > 31 {
                bail!("service {name} pty must be 0..=31");
            }
            short_label_mask(
                &format!("service {name} short_label"),
                &service.label,
                service.short_label.as_deref(),
            )?;
            if service.id > u32::from(u16::MAX) {
                bail!("service {name} id exceeds 16-bit programme service range");
            }
            if !service_ids.insert(service.id) {
                bail!("duplicate service id: {name}");
            }
        }

        let mut subchannel_ids = HashSet::new();
        let mut input_endpoints = HashSet::new();
        let mut tcp_input_ports = HashSet::new();
        let mut validated = Vec::with_capacity(self.subchannels.len());
        let mut next_cu: u16 = 0;
        let mut payload_words = 0usize;
        for sub in &self.subchannels {
            let name = &sub.name;
            if sub.id >= 64 || !subchannel_ids.insert(sub.id) {
                bail!("invalid or duplicate subchannel id {} on {name}", sub.id);
            }
            if sub.bitrate == 0 || !sub.bitrate.is_multiple_of(8) {
                bail!("subchannel {name} bitrate must be a positive multiple of 8");
            }
            let (size, tpl) = sub
                .protection
                .size_and_tpl(sub.bitrate)
                .with_context(|| format!("subchannel {name} protection"))?;
            let end = next_cu.checked_add(size).context("CU address overflow")?;
            if end > 864 {
                bail!("subchannels exceed 864 CU at {name}");
            }
            let bytes = usize::from(sub.bitrate) * 3;
            payload_words += bytes / 4;
            let endpoint = sub
                .input
                .endpoint()
                .with_context(|| format!("subchannel {name} input"))?;
            if !input_endpoints.insert(endpoint) {
                bail!("duplicate input endpoint: {}", sub.input.uri());
            }
            if endpoint.transport == Transport::Tcp {
                tcp_input_ports.insert(endpoint.address.port());
            } else if matches!(
                sub.input,
                InputConfig::Edi {
                    backpressure: Some(true),
                    ..
                }
            ) {
                bail!("subchannel {name} backpressure requires a TCP input");
            }
            if let InputConfig::Edi {
                stream_index,
                buffer_frames,
                prebuffer_frames,
                timing,
                ..
            } = &sub.input
            {
                if *stream_index == 0
                    || *buffer_frames == 0
                    || *buffer_frames > 1000
                    || *prebuffer_frames == 0
                    || prebuffer_frames > buffer_frames
                {
                    bail!("subchannel {name} has invalid EDI stream or buffer settings");
                }
                if *timing == InputTiming::Timestamped && !self.ensemble.tist {
                    bail!("subchannel {name} timestamped input requires ensemble.tist");
                }
            }
            validated.push(ValidatedSubchannel {
                name: name.clone(),
                id: sub.id,
                bitrate: sub.bitrate,
                start_address_cu: next_cu,
                size_cu: size,
                payload_bytes: bytes,
                tpl,
                endpoint,
            });
            next_cu = end;
        }

        let mut components = Vec::with_capacity(self.components.len());
        let mut subchannel_languages = std::collections::HashMap::new();
        for component in &self.components {
            let service = &self.services[component.service];
            let name = service_name(service.id);
            let scids = components
                .iter()
                .filter(|c: &&ValidatedComponent| c.service == component.service)
                .count();
            if scids >= 12 {
                bail!("service {name} has too many components for FIG 0/2");
            }
            if component.user_applications.len() > 1 {
                bail!("service {name} component {scids} supports one slideshow application");
            }
            // FIG 0/13 identifies secondary components by SCIdS, which needs
            // FIG 0/8; only the primary component is addressable without it.
            if scids > 0 && !component.user_applications.is_empty() {
                bail!("service {name} user applications are only supported on its first component");
            }
            // FIG 0/5 signals language per subchannel.
            if service.language != 0 {
                let previous = subchannel_languages.insert(component.subchannel, service.language);
                if previous.is_some_and(|previous| previous != service.language) {
                    bail!(
                        "subchannel {} is shared by services with different languages",
                        self.subchannels[component.subchannel].name
                    );
                }
            }
            components.push(ValidatedComponent {
                service: component.service,
                subchannel: component.subchannel,
                scids: scids as u8,
            });
        }
        for (index, service) in self.services.iter().enumerate() {
            if !components.iter().any(|c| c.service == index) {
                bail!("service {} has no component", service_name(service.id));
            }
        }
        let mut tcp_output_ports = HashSet::new();
        for destination in &self.output.destinations {
            match destination {
                EdiDestination::Udp { address, port } => {
                    if *port == 0 || address.parse::<std::net::IpAddr>().is_err() {
                        bail!("EDI UDP destination needs an IP address and nonzero port");
                    }
                }
                EdiDestination::Tcp { listen_port, .. } if *listen_port == 0 => {
                    bail!("EDI TCP listen_port must be nonzero")
                }
                EdiDestination::Tcp { listen_port, .. }
                    if tcp_input_ports.contains(listen_port) =>
                {
                    bail!("EDI TCP output port conflicts with an input endpoint")
                }
                EdiDestination::Tcp {
                    listen_port,
                    max_frames_queued,
                    preroll_ms,
                } => {
                    if !(1..=10_000).contains(max_frames_queued)
                        || *preroll_ms > 30_000
                        || usize::try_from(preroll_ms.div_ceil(24))? > *max_frames_queued
                    {
                        bail!("invalid EDI TCP queue or preroll setting");
                    }
                    if !tcp_output_ports.insert(*listen_port) {
                        bail!("duplicate EDI TCP output port: {listen_port}");
                    }
                }
            }
        }
        let fic_words = if mode == 3 { 32 } else { 24 };
        let frame_words = 1 + fic_words + validated.len() + payload_words;
        if (frame_words + 4) * 4 > 6144 {
            bail!("ETI frame exceeds 6144 bytes");
        }
        Ok(ValidatedConfig {
            source: self,
            subchannels: validated,
            components,
            fic_words,
            frame_words,
        })
    }
}

impl ProtectionConfig {
    fn size_and_tpl(&self, bitrate: u16) -> anyhow::Result<(u16, u8)> {
        let (numerator, denominator, option, level) = match self {
            Self::EepA { level } => (
                [12, 8, 6, 4][usize::from(
                    level
                        .checked_sub(1)
                        .filter(|v| *v < 4)
                        .context("EEP-A level must be 1..=4")?,
                )],
                8,
                0,
                *level,
            ),
            Self::EepB { level } => {
                if !bitrate.is_multiple_of(32) {
                    bail!("EEP-B bitrate must be divisible by 32");
                }
                (
                    [27, 21, 18, 15][usize::from(
                        level
                            .checked_sub(1)
                            .filter(|v| *v < 4)
                            .context("EEP-B level must be 1..=4")?,
                    )],
                    32,
                    1,
                    *level,
                )
            }
        };
        let size = u32::from(bitrate) * numerator / denominator;
        Ok((u16::try_from(size)?, 0x20 | (option << 2) | (level - 1)))
    }
}

/// Encode a label for FIG 1 (EBU Latin, one byte per character).
pub fn encode_label(path: &str, label: &str) -> anyhow::Result<Vec<u8>> {
    let encoded = dabmux::charset::encode(label).map_err(|err| anyhow::anyhow!("{path}: {err}"))?;
    if encoded.is_empty() || encoded.len() > 16 {
        bail!("{path} must contain 1..=16 EBU Latin characters");
    }
    Ok(encoded)
}

fn validate_label(path: &str, label: &str) -> anyhow::Result<()> {
    encode_label(path, label).map(drop)
}

pub fn short_label_mask(path: &str, label: &str, short_label: Option<&str>) -> anyhow::Result<u16> {
    let label = encode_label(path, label)?;
    let Some(short_label) = short_label else {
        return Ok(0xff00);
    };
    let short_label =
        dabmux::charset::encode(short_label).map_err(|err| anyhow::anyhow!("{path}: {err}"))?;
    if short_label.is_empty() || short_label.len() > 8 {
        bail!("{path} must contain 1..=8 characters from the full label");
    }
    let mut remaining = short_label.into_iter();
    let mut wanted = remaining.next();
    let mut mask = 0u16;
    for (position, character) in label.into_iter().enumerate() {
        if Some(character) == wanted {
            mask |= 0x8000 >> position;
            wanted = remaining.next();
            if wanted.is_none() {
                return Ok(mask);
            }
        }
    }
    bail!("{path} must be a character subsequence of the full label")
}

pub fn load_from_file(path: &Path) -> anyhow::Result<ValidatedConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config {}", path.display()))?;
    parse_yaml(&raw)
        .with_context(|| format!("failed to parse config {}", path.display()))?
        .validate()
        .with_context(|| format!("invalid config {}", path.display()))
}

/// Parse YAML config; errors name the offending field, e.g.
/// `services[2].components[0].input: missing field `uri``.
pub fn parse_yaml(raw: &str) -> anyhow::Result<Config> {
    serde_path_to_error::deserialize(serde_norway::Deserializer::from_str(raw))
        .map_err(|err| anyhow::anyhow!("{}: {}", err.path(), err.inner()))
}

pub fn parse_json(raw: &[u8]) -> anyhow::Result<Config> {
    serde_path_to_error::deserialize(&mut serde_json::Deserializer::from_slice(raw))
        .map_err(|err| anyhow::anyhow!("{}: {}", err.path(), err.inner()))
}

pub fn resolve_path(path: PathBuf) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

/// Builders shared by tests in this and other modules.
#[cfg(test)]
pub mod testing {
    use super::schema::*;
    use super::*;

    pub fn example() -> Config {
        parse_yaml(include_str!("../tests/fixtures/minimal.yaml")).unwrap()
    }

    /// A DAB+ component with its own subchannel fed by EDI.
    pub fn dab_plus(bitrate: u16, uri: &str) -> ComponentConfig {
        ComponentConfig {
            kind: Some(SubchannelKind::DabPlus),
            bitrate: Some(bitrate),
            input: Some(InputSpec::Edi {
                uri: uri.into(),
                stream_index: None,
                buffer_frames: None,
                prebuffer_frames: None,
                timing: None,
                backpressure: None,
            }),
            ..Default::default()
        }
    }

    pub fn reference(name: &str) -> ComponentConfig {
        ComponentConfig {
            subchannel: Some(name.into()),
            ..Default::default()
        }
    }

    pub fn service(id: u32, label: &str, components: Vec<ComponentConfig>) -> ServiceConfig {
        ServiceConfig {
            id,
            label: label.into(),
            short_label: None,
            pty: 0,
            language: 0,
            components,
        }
    }

    /// The full error chain, including validation context.
    pub fn error(config: Config) -> String {
        format!("{:#}", config.validate().unwrap_err())
    }
}

#[cfg(test)]
mod tests {
    use super::schema::*;
    use super::testing::*;
    use super::*;

    #[test]
    fn example_layout_and_eep_a() {
        let mut config = example();
        let mut second = dab_plus(64, "udp://127.0.0.1:9002");
        second.protection = Some(ProtectionConfig::EepA { level: 2 });
        config.services[0].components.push(second);
        let valid = config.validate().unwrap();
        assert_eq!(valid.subchannels[0].start_address_cu, 0);
        assert_eq!(valid.subchannels[0].size_cu, 72);
        assert_eq!(valid.subchannels[0].payload_bytes, 288);
        assert_eq!(valid.subchannels[0].tpl, 0x22, "EEP 3-A by default");
        assert_eq!(valid.subchannels[1].start_address_cu, 72);
        assert_eq!(valid.subchannels[1].size_cu, 64);
        assert_eq!(valid.frame_words, 1 + 24 + 2 + 72 + 48);
        let names: Vec<_> = valid.subchannels.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["4DA4/0", "4DA4/1"]);
        // ID 1 is configured, so the second subchannel takes the lowest free ID.
        assert_eq!(valid.subchannels[1].id, 0);
        assert!(!valid.source.subchannels[0].allocated && valid.source.subchannels[1].allocated);
    }

    #[test]
    fn capacity_and_subchannel_id_range_are_checked() {
        let mut config = example();
        config.services[0].components[0].bitrate = Some(1200);
        assert!(error(config).contains("864 CU"));

        let mut config = example();
        config.services[0].components[0].subchannel_id = Some(64);
        assert!(error(config).contains("must be 0..=63"));
    }

    #[test]
    fn eep_b_requires_valid_level_and_bitrate() {
        let mut config = example();
        config.services[0].components[0].protection = Some(ProtectionConfig::EepB { level: 1 });
        let valid = config.clone().validate().unwrap();
        assert_eq!(valid.subchannels[0].size_cu, 81);
        assert_eq!(valid.subchannels[0].tpl, 0x24);
        config.services[0].components[0].bitrate = Some(88);
        assert!(error(config).contains("divisible by 32"));

        let mut config = example();
        config.services[0].components[0].protection = Some(ProtectionConfig::EepA { level: 5 });
        assert!(error(config).contains("level must be 1..=4"));
    }

    #[test]
    fn duplicate_bindings_are_rejected_before_activation() {
        let mut config = example();
        config.services.push(service(
            0x4da5,
            "Radio Two",
            vec![dab_plus(96, "udp://127.0.0.1:9000")],
        ));
        assert!(error(config).contains("duplicate input endpoint"));

        let mut config = example();
        let tcp = EdiDestination::Tcp {
            listen_port: 9001,
            max_frames_queued: 500,
            preroll_ms: 0,
        };
        config.output.destinations = vec![tcp.clone(), tcp];
        assert!(error(config).contains("duplicate EDI TCP output port"));

        let mut config = example();
        config.services.push(config.services[0].clone());
        assert!(error(config).contains("duplicate service id: 4DA4"));
    }

    #[test]
    fn short_label_is_a_character_mask_not_a_second_text_field() {
        assert_eq!(
            short_label_mask("service.short_label", "105 DJ HRND-001", Some("HRND-001")).unwrap(),
            0x01fe
        );
        assert_eq!(
            short_label_mask("ensemble.short_label", "RND D00 - XX", Some("RND D00")).unwrap(),
            0xfe00
        );
        assert_eq!(
            short_label_mask("short_label", "Radio One", None).unwrap(),
            0xff00
        );
        assert!(short_label_mask("short_label", "HELIUM RND-004", Some("HRND-004")).is_ok());
        assert!(short_label_mask("short_label", "Radio One", Some("Other")).is_err());
        assert!(short_label_mask("short_label", "Radio One", Some("Radio One")).is_err());
    }

    /// Two services on one named subchannel.
    fn shared(language_one: u8, language_two: u8) -> Config {
        let mut config = example();
        config.services[0].components = vec![reference("shared_audio")];
        config.services[0].language = language_one;
        let mut two = service(0x4da5, "Radio Two", vec![reference("shared_audio")]);
        two.language = language_two;
        config.services.push(two);
        config.subchannels.insert(
            "shared_audio".into(),
            SubchannelConfig {
                id: Some(1),
                kind: SubchannelKind::DabPlus,
                bitrate: 96,
                protection: None,
                input: InputSpec::Sti {
                    uri: "rtp://127.0.0.1:9000".into(),
                },
            },
        );
        config
    }

    #[test]
    fn shared_subchannel_is_one_subchannel_for_both_services() {
        let valid = shared(8, 8).validate().unwrap();
        assert_eq!(valid.subchannels.len(), 1);
        assert_eq!(valid.subchannels[0].name, "shared_audio");
        assert_eq!(
            valid.components,
            [
                ValidatedComponent {
                    service: 0,
                    subchannel: 0,
                    scids: 0
                },
                ValidatedComponent {
                    service: 1,
                    subchannel: 0,
                    scids: 0
                },
            ]
        );
        assert!(shared(8, 0).validate().is_ok());
        assert!(error(shared(8, 9)).contains("different languages"));
    }

    #[test]
    fn shared_subchannel_references_are_strict() {
        let mut config = shared(0, 0);
        config.services[1].components[0].subchannel = Some("missing".into());
        assert_eq!(
            error(config),
            "services[1] (4DA5).components[0] refers to unknown subchannel \"missing\""
        );

        let mut config = shared(0, 0);
        config.services[1].components[0].bitrate = Some(64);
        assert!(error(config).contains(
            "set its type, bitrate, protection, input and id under subchannels.shared_audio"
        ));

        let mut config = shared(0, 0);
        let unused = config.subchannels["shared_audio"].clone();
        config.subchannels.insert("spare".into(), unused);
        assert!(error(config).contains("subchannels.spare is not used by any component"));

        let mut config = example();
        config.services[0].components[0].input = None;
        assert!(error(config).contains(
            "services[0] (4DA4).components[0] needs type, bitrate and input, or a reference"
        ));

        let mut config = example();
        config.services[0].components.clear();
        assert!(error(config).contains("needs at least one component"));
    }

    #[test]
    fn subchannels_follow_first_use_and_ids_avoid_configured_ones() {
        let mut config = example();
        config.services[0].components[0].subchannel_id = Some(0);
        config.services.push(service(
            0x4da5,
            "Radio Two",
            vec![dab_plus(64, "udp://127.0.0.1:9002")],
        ));
        config.services.push(service(
            0x4da6,
            "Radio Three",
            vec![dab_plus(48, "udp://127.0.0.1:9003")],
        ));
        let valid = config.clone().validate().unwrap();
        let ids: Vec<_> = valid.subchannels.iter().map(|s| s.id).collect();
        assert_eq!(ids, [0, 1, 2]);

        config.services[2].components[0].subchannel_id = Some(1);
        let valid = config.clone().validate().unwrap();
        let ids: Vec<_> = valid.subchannels.iter().map(|s| s.id).collect();
        assert_eq!(ids, [0, 2, 1]);

        config.services[1].components[0].subchannel_id = Some(1);
        assert_eq!(
            error(config),
            "subchannel id 1 is set on both 4DA5/0 and 4DA6/0"
        );
    }

    #[test]
    fn inserting_a_service_reports_reallocated_ids() {
        let mut config = example();
        config.services.push(service(
            0x4da5,
            "Radio Two",
            vec![dab_plus(64, "udp://127.0.0.1:9002")],
        ));
        let before = config.clone().validate().unwrap();
        config.services.insert(
            0,
            service(
                0x4da6,
                "Radio New",
                vec![dab_plus(48, "udp://127.0.0.1:9003")],
            ),
        );
        let after = config.validate().unwrap();
        // Radio One's configured ID 1 stays; Radio Two moves from 0 to 2.
        assert_eq!(
            after.reallocated_subchannels(&before),
            [("4DA5/0".to_string(), 0, 2)]
        );
    }

    #[test]
    fn defaults_fill_unset_protection_and_edi_settings() {
        let config = parse_yaml(
            r#"
ensemble: {id: 0x4fff, ecc: 0xe1, label: Example Mux}
defaults:
  protection: {profile: eep_a, level: 2}
  edi: {buffer_frames: 100, prebuffer_frames: 30, backpressure: false}
services:
  - id: 0x4da4
    label: Radio One
    components:
      - type: dab_plus
        bitrate: 96
        input: {protocol: edi, uri: "tcp://:9000"}
      - type: dab_plus
        bitrate: 64
        protection: {profile: eep_a, level: 3}
        input: {protocol: edi, uri: "udp://:9001", buffer_frames: 50}
output:
  destinations: [{protocol: tcp, listen_port: 9100}]
"#,
        )
        .unwrap();
        let valid = config.validate().unwrap();
        let subs = &valid.source.subchannels;
        assert_eq!(subs[0].protection, ProtectionConfig::EepA { level: 2 });
        assert_eq!(subs[1].protection, ProtectionConfig::EepA { level: 3 });
        assert_eq!(
            subs[0].input,
            InputConfig::Edi {
                uri: "tcp://:9000".into(),
                stream_index: 1,
                buffer_frames: 100,
                prebuffer_frames: 30,
                timing: InputTiming::Prebuffering,
                backpressure: Some(false),
            }
        );
        // The backpressure default applies to TCP inputs only.
        assert!(matches!(
            subs[1].input,
            InputConfig::Edi {
                buffer_frames: 50,
                prebuffer_frames: 30,
                backpressure: None,
                ..
            }
        ));
    }

    #[test]
    fn ids_accept_hex_strings_and_parse_errors_name_the_field() {
        let yaml = include_str!("../tests/fixtures/minimal.yaml")
            .replace("id: 0x4da4", "id: 0X4DA4")
            .replace("ecc: 0xe1", "ecc: \"0xE1\"");
        let config = parse_yaml(&yaml).unwrap();
        assert_eq!((config.services[0].id, config.ensemble.ecc), (0x4da4, 0xe1));

        let json = serde_json::json!({
            "ensemble": {"id": "0x4FFF", "ecc": 225, "label": "Example Mux"},
            "services": [{"id": "0x4DA4", "label": "Radio One", "language": "0x08",
                "components": [{"type": "dab_plus", "bitrate": 96, "subchannel_id": "7",
                    "input": {"protocol": "sti", "uri": "rtp://:9000"}}]}],
            "output": {"destinations": [{"protocol": "tcp", "listen_port": 9100}]}
        });
        let config = parse_json(json.to_string().as_bytes()).unwrap();
        assert_eq!(config.services[0].language, 8);
        assert_eq!(config.services[0].components[0].subchannel_id, Some(7));

        let err = parse_yaml(&yaml.replace("id: 0X4DA4", "id: 0x1FFFFFFFF")).unwrap_err();
        assert!(err.to_string().starts_with("services[0].id: "), "{err}");
        let err = parse_yaml(&yaml.replace("uri: udp", "url: udp")).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("services[0].components[0].input: "),
            "{err}"
        );
    }

    #[test]
    fn user_applications_only_on_primary_component() {
        let mut config = example();
        let mut second = dab_plus(64, "udp://127.0.0.1:9002");
        second.user_applications = vec![UserApplication::Slideshow];
        config.services[0].components.push(second);
        assert!(error(config.clone()).contains("only supported on its first component"));
        config.services[0].components[1].user_applications.clear();
        let valid = config.validate().unwrap();
        assert_eq!(
            valid.components,
            [
                ValidatedComponent {
                    service: 0,
                    subchannel: 0,
                    scids: 0
                },
                ValidatedComponent {
                    service: 0,
                    subchannel: 1,
                    scids: 1
                },
            ]
        );
    }

    #[test]
    fn input_endpoints_are_resolved_once() {
        let edi = |uri: &str, backpressure: Option<bool>| InputConfig::Edi {
            uri: uri.into(),
            stream_index: 1,
            buffer_frames: 40,
            prebuffer_frames: 4,
            timing: InputTiming::Prebuffering,
            backpressure,
        };
        assert_eq!(
            edi("tcp://:9000", None).endpoint().unwrap().to_string(),
            "tcp://0.0.0.0:9000"
        );
        assert_eq!(
            InputConfig::Sti {
                uri: "rtp://127.0.0.1:9002".into()
            }
            .endpoint()
            .unwrap()
            .transport,
            Transport::Udp
        );
        assert!(edi("http://127.0.0.1:9000", None).endpoint().is_err());
        assert!(edi("udp://localhost:9000", None).endpoint().is_err());

        assert!(edi("tcp://127.0.0.1:9000", None).backpressure());
        assert!(!edi("tcp://127.0.0.1:9000", Some(false)).backpressure());
        assert!(!edi("udp://127.0.0.1:9000", None).backpressure());

        let mut config = example();
        if let Some(InputSpec::Edi { backpressure, .. }) =
            &mut config.services[0].components[0].input
        {
            *backpressure = Some(true);
        }
        assert!(error(config).contains("backpressure requires a TCP input"));
    }

    #[test]
    fn labels_are_ebu_latin_not_ascii() {
        assert_eq!(
            short_label_mask("short_label", "Radio Zürich 1", Some("Zürich")).unwrap(),
            0x03f0
        );
        assert!(validate_label("service.label", "Grüezi Gämsli").is_ok());
        assert!(validate_label("service.label", "Sechzehn Zeichenü").is_err());
        let err = validate_label("service.label", "Radio ~").unwrap_err();
        assert!(err.to_string().contains("service.label"));
        assert!(err.to_string().contains("EBU Latin"));
    }

    #[test]
    fn supplied_production_configs_convert_to_valid_rust_yaml() {
        let valid = parse_yaml(include_str!("../config.production.example.yaml"))
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(valid.source.services.len(), 12);
        assert_eq!(valid.source.components.len(), 12);
        assert_eq!(valid.subchannels.len(), 12);
        assert_eq!(
            valid.subchannels.iter().map(|sub| sub.size_cu).sum::<u16>(),
            552
        );
        assert_eq!(valid.source.ensemble.tist_offset_ms, 2000);
        assert_eq!(valid.source.output.tagpacket_alignment, 16);

        let valid = parse_yaml(include_str!("../config.mux-zh.example.yaml"))
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(valid.source.services.len(), 17);
        assert_eq!(
            valid.subchannels.iter().map(|sub| sub.size_cu).sum::<u16>(),
            822
        );
        assert!(valid.source.subchannels.iter().all(|sub| !sub.allocated));
        assert!(matches!(
            valid.source.subchannels[0].input,
            InputConfig::Edi {
                buffer_frames: 100,
                prebuffer_frames: 30,
                ..
            }
        ));
    }

    #[test]
    fn resolved_view_shows_allocation_and_layout() {
        let mut config = example();
        config.services[0]
            .components
            .push(dab_plus(64, "udp://127.0.0.1:9002"));
        let valid = config.validate().unwrap();
        let resolved = serde_json::to_value(valid.resolved()).unwrap();
        assert_eq!(resolved["services"][0]["id"], 0x4da4);
        assert_eq!(
            resolved["services"][0]["components"][1]["subchannel"],
            "4DA4/1"
        );
        let second = &resolved["subchannels"][1];
        assert_eq!(
            (
                &second["id"],
                &second["allocated"],
                &second["start_address_cu"]
            ),
            (&0.into(), &true.into(), &72.into())
        );
        assert_eq!(second["protection"]["profile"], "eep_a");
        assert_eq!(second["input"]["buffer_frames"], 40);
    }

    #[tokio::test]
    async fn invalid_candidate_does_not_replace_active_config() {
        let initial = example().validate().unwrap();
        let shared = SharedConfig::new(initial.clone());
        let mut invalid = example();
        invalid.services[0].components.clear();
        assert!(invalid.validate().is_err());
        assert_eq!(*shared.read().await, initial);

        let (tx, mut rx) = mpsc::channel(1);
        shared.install_updates(tx).await;
        let worker = shared.clone();
        tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                worker.commit(update.candidate).await;
                let _ = update.reply.send(Ok(true));
            }
        });
        let mut changed = example();
        changed.services[0].label = "New Label".into();
        assert!(shared.apply(changed).await.unwrap());
        assert_eq!(shared.read().await.source.services[0].label, "New Label");
        let mut layout_change = example();
        layout_change.services[0].components[0].bitrate = Some(104);
        assert!(shared.apply(layout_change).await.unwrap());
        assert_eq!(shared.read().await.subchannels[0].bitrate, 104);
    }
}
