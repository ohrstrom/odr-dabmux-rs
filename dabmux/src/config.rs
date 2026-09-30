use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

pub mod reload;
pub mod watch;

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

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub ensemble: EnsembleConfig,
    pub services: Vec<ServiceConfig>,
    pub subchannels: Vec<SubchannelConfig>,
    pub components: Vec<ComponentConfig>,
    pub output: EdiOutputConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnsembleConfig {
    pub id: u16,
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
    pub tai_clock_bulletins: Option<String>,
}

fn default_mode() -> u8 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub uid: String,
    pub id: u32,
    pub label: String,
    pub short_label: Option<String>,
    #[serde(default)]
    pub pty: u8,
    #[serde(default)]
    pub language: u8,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentConfig {
    pub uid: String,
    pub service: String,
    pub subchannel: String,
    #[serde(default)]
    pub user_applications: Vec<UserApplication>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UserApplication {
    Slideshow,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubchannelConfig {
    pub uid: String,
    pub id: u8,
    pub bitrate: u16,
    pub kind: SubchannelKind,
    pub protection: ProtectionConfig,
    pub input: InputConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubchannelKind {
    DabPlus,
    MpegAudio,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "profile", deny_unknown_fields)]
pub enum ProtectionConfig {
    EepA { level: u8 },
    EepB { level: u8 },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "protocol", deny_unknown_fields)]
pub enum InputConfig {
    Edi {
        uri: String,
        #[serde(default = "default_stream_index")]
        stream_index: u16,
        #[serde(default = "default_buffer_frames")]
        buffer_frames: usize,
        #[serde(default = "default_prebuffer_frames")]
        prebuffer_frames: usize,
        #[serde(default)]
        timing: InputTiming,
        /// TCP only, on by default: stop reading while the buffer is full so
        /// the producer is throttled, which unpaced file encoders need. Set
        /// `false` for live encoders to drop the oldest frame instead, so that
        /// clock drift cannot grow latency without bound.
        #[serde(default)]
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

fn default_stream_index() -> u16 {
    1
}
fn default_buffer_frames() -> usize {
    40
}
fn default_prebuffer_frames() -> usize {
    4
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputTiming {
    #[default]
    Prebuffering,
    Timestamped,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EdiOutputConfig {
    pub destinations: Vec<EdiDestination>,
    #[serde(default = "default_tagpacket_alignment")]
    pub tagpacket_alignment: u8,
}

fn default_tagpacket_alignment() -> u8 {
    8
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
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
    pub source: Config,
    pub subchannels: Vec<ValidatedSubchannel>,
    /// Components in config order, with references resolved to indices.
    pub components: Vec<ValidatedComponent>,
    pub fic_words: usize,
    pub frame_words: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSubchannel {
    pub uid: String,
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
}

impl Config {
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
            && self.ensemble.tai_clock_bulletins.is_none()
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
        if let Some(urls) = &self.ensemble.tai_clock_bulletins {
            if urls
                .split('|')
                .any(|url| !url.starts_with("https://") || url.len() < 10)
            {
                bail!("tai_clock_bulletins must contain HTTPS URLs separated by pipes");
            }
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
        if self.subchannels.is_empty() {
            bail!("at least one subchannel is required");
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

        let mut service_uids = HashSet::new();
        let mut service_ids = HashSet::new();
        for service in &self.services {
            validate_uid("service", &service.uid)?;
            validate_label("service.label", &service.label)?;
            if service.pty > 31 {
                bail!("service {} pty must be 0..=31", service.uid);
            }
            short_label_mask(
                "service.short_label",
                &service.label,
                service.short_label.as_deref(),
            )?;
            if service.id > u16::MAX as u32 {
                bail!(
                    "service {} id exceeds 16-bit programme service range",
                    service.uid
                );
            }
            if !service_uids.insert(service.uid.as_str()) {
                bail!("duplicate service uid: {}", service.uid);
            }
            if !service_ids.insert(service.id) {
                bail!("duplicate service id: {}", service.id);
            }
        }

        let mut subchannel_uids = HashSet::new();
        let mut subchannel_ids = HashSet::new();
        let mut input_endpoints = HashSet::new();
        let mut tcp_input_ports = HashSet::new();
        let mut validated = Vec::with_capacity(self.subchannels.len());
        let mut next_cu: u16 = 0;
        let mut payload_words = 0usize;
        for sub in &self.subchannels {
            validate_uid("subchannel", &sub.uid)?;
            if !subchannel_uids.insert(sub.uid.as_str()) {
                bail!("duplicate subchannel uid: {}", sub.uid);
            }
            if sub.id >= 64 || !subchannel_ids.insert(sub.id) {
                bail!("invalid or duplicate subchannel id: {}", sub.id);
            }
            if sub.bitrate == 0 || !sub.bitrate.is_multiple_of(8) {
                bail!(
                    "subchannel {} bitrate must be a positive multiple of 8",
                    sub.uid
                );
            }
            let (size, tpl) = sub.protection.size_and_tpl(sub.bitrate)?;
            let end = next_cu.checked_add(size).context("CU address overflow")?;
            if end > 864 {
                bail!("subchannels exceed 864 CU at {}", sub.uid);
            }
            let bytes = usize::from(sub.bitrate) * 3;
            payload_words += bytes / 4;
            let endpoint = sub
                .input
                .endpoint()
                .with_context(|| format!("subchannel {} input", sub.uid))?;
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
                bail!("subchannel {} backpressure requires a TCP input", sub.uid);
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
                    bail!(
                        "subchannel {} has invalid EDI stream or buffer settings",
                        sub.uid
                    );
                }
                if *timing == InputTiming::Timestamped && !self.ensemble.tist {
                    bail!(
                        "subchannel {} timestamped input requires ensemble.tist",
                        sub.uid
                    );
                }
            }
            validated.push(ValidatedSubchannel {
                uid: sub.uid.clone(),
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

        let mut component_uids = HashSet::new();
        let mut linked_services = HashSet::new();
        let mut components = Vec::with_capacity(self.components.len());
        let mut subchannel_languages = std::collections::HashMap::new();
        for component in &self.components {
            validate_uid("component", &component.uid)?;
            if !component_uids.insert(component.uid.as_str()) {
                bail!("duplicate component uid: {}", component.uid);
            }
            if !service_uids.contains(component.service.as_str()) {
                bail!(
                    "component {} refers to unknown service {}",
                    component.uid,
                    component.service
                );
            }
            if !subchannel_uids.contains(component.subchannel.as_str()) {
                bail!(
                    "component {} refers to unknown subchannel {}",
                    component.uid,
                    component.subchannel
                );
            }
            if component.user_applications.len() > 1 {
                bail!(
                    "component {} supports one slideshow application",
                    component.uid
                );
            }
            let service = self
                .services
                .iter()
                .position(|s| s.uid == component.service)
                .expect("checked service reference");
            let subchannel = self
                .subchannels
                .iter()
                .position(|s| s.uid == component.subchannel)
                .expect("checked subchannel reference");
            let scids = components
                .iter()
                .filter(|c: &&ValidatedComponent| c.service == service)
                .count();
            // FIG 0/13 identifies secondary components by SCIdS, which needs
            // FIG 0/8; only the primary component is addressable without it.
            if scids > 0 && !component.user_applications.is_empty() {
                bail!(
                    "component {} user applications are only supported on a service's first component",
                    component.uid
                );
            }
            // FIG 0/5 signals language per subchannel.
            let language = self.services[service].language;
            if language != 0 {
                let previous = subchannel_languages.insert(subchannel, language);
                if previous.is_some_and(|previous| previous != language) {
                    bail!(
                        "subchannel {} is shared by services with different languages",
                        component.subchannel
                    );
                }
            }
            linked_services.insert(component.service.as_str());
            components.push(ValidatedComponent {
                service,
                subchannel,
                scids: scids as u8,
            });
        }
        for service in &self.services {
            if !linked_services.contains(service.uid.as_str()) {
                bail!("service {} has no component", service.uid);
            }
            let count = self
                .components
                .iter()
                .filter(|c| c.service == service.uid)
                .count();
            if count > 12 {
                bail!(
                    "service {} has too many components for FIG 0/2",
                    service.uid
                );
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

fn validate_uid(kind: &str, uid: &str) -> anyhow::Result<()> {
    if uid.trim().is_empty() {
        bail!("{kind} uid must not be empty");
    }
    Ok(())
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
    let config: Config = serde_yaml::from_str(&raw)
        .with_context(|| format!("failed to parse config {}", path.display()))?;
    config
        .validate()
        .with_context(|| format!("invalid config {}", path.display()))
}

pub fn resolve_path(path: PathBuf) -> anyhow::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Config {
        serde_yaml::from_str(include_str!("../tests/fixtures/minimal.yaml")).unwrap()
    }

    #[test]
    fn example_layout_and_eep_a() {
        let mut config = example();
        config.subchannels.push(SubchannelConfig {
            uid: "audio_two".into(),
            id: 2,
            bitrate: 64,
            kind: SubchannelKind::DabPlus,
            protection: ProtectionConfig::EepA { level: 2 },
            input: InputConfig::Sti {
                uri: "rtp://127.0.0.1:9002".into(),
            },
        });
        config.components.push(ComponentConfig {
            uid: "component_two".into(),
            service: "radio_one".into(),
            subchannel: "audio_two".into(),
            user_applications: Vec::new(),
        });
        let valid = config.validate().unwrap();
        assert_eq!(valid.subchannels[0].start_address_cu, 0);
        assert_eq!(valid.subchannels[0].size_cu, 72);
        assert_eq!(valid.subchannels[0].payload_bytes, 288);
        assert_eq!(valid.subchannels[0].tpl, 0x22);
        assert_eq!(valid.subchannels[1].start_address_cu, 72);
        assert_eq!(valid.subchannels[1].size_cu, 64);
        assert_eq!(valid.frame_words, 1 + 24 + 2 + 72 + 48);
    }

    #[test]
    fn invalid_graph_and_capacity_are_rejected() {
        let mut config = example();
        config.components[0].subchannel = "missing".into();
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("unknown subchannel"));

        let mut config = example();
        config.subchannels[0].bitrate = 1200;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("864 CU"));

        let mut config = example();
        config.subchannels[0].id = 64;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("subchannel id"));
    }

    #[test]
    fn eep_b_requires_valid_level_and_bitrate() {
        let mut config = example();
        config.subchannels[0].protection = ProtectionConfig::EepB { level: 1 };
        let valid = config.clone().validate().unwrap();
        assert_eq!(valid.subchannels[0].size_cu, 81);
        assert_eq!(valid.subchannels[0].tpl, 0x24);
        config.subchannels[0].bitrate = 88;
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("divisible by 32"));

        let mut config = example();
        config.subchannels[0].protection = ProtectionConfig::EepA { level: 5 };
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("level must be 1..=4"));
    }

    #[test]
    fn duplicate_bindings_are_rejected_before_activation() {
        let mut config = example();
        let mut second = config.subchannels[0].clone();
        second.uid = "audio_two".into();
        second.id = 2;
        config.subchannels.push(second);
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate input endpoint"));

        let mut config = example();
        config.output.destinations = vec![
            EdiDestination::Tcp {
                listen_port: 9001,
                max_frames_queued: 500,
                preroll_ms: 0,
            },
            EdiDestination::Tcp {
                listen_port: 9001,
                max_frames_queued: 500,
                preroll_ms: 0,
            },
        ];
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("duplicate EDI TCP output port"));
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

    fn with_second_service_on_shared_subchannel(language: u8) -> Config {
        let mut config = example();
        config.services[0].language = 8;
        config.services.push(ServiceConfig {
            uid: "radio_two".into(),
            id: 0x4da5,
            label: "Radio Two".into(),
            short_label: None,
            pty: 0,
            language,
        });
        config.components.push(ComponentConfig {
            uid: "component_two".into(),
            service: "radio_two".into(),
            subchannel: "audio_one".into(),
            user_applications: Vec::new(),
        });
        config
    }

    #[test]
    fn shared_subchannel_languages_must_agree() {
        assert!(with_second_service_on_shared_subchannel(8)
            .validate()
            .is_ok());
        assert!(with_second_service_on_shared_subchannel(0)
            .validate()
            .is_ok());
        assert!(with_second_service_on_shared_subchannel(9)
            .validate()
            .unwrap_err()
            .to_string()
            .contains("different languages"));
    }

    #[test]
    fn user_applications_only_on_primary_component() {
        let mut config = example();
        config.subchannels.push(SubchannelConfig {
            uid: "audio_two".into(),
            id: 2,
            bitrate: 64,
            kind: SubchannelKind::DabPlus,
            protection: ProtectionConfig::EepA { level: 3 },
            input: InputConfig::Sti {
                uri: "rtp://127.0.0.1:9002".into(),
            },
        });
        config.components.push(ComponentConfig {
            uid: "component_two".into(),
            service: "radio_one".into(),
            subchannel: "audio_two".into(),
            user_applications: vec![UserApplication::Slideshow],
        });
        assert!(config
            .clone()
            .validate()
            .unwrap_err()
            .to_string()
            .contains("first component"));
        config.components[1].user_applications.clear();
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
        config.subchannels[0].input = edi("udp://127.0.0.1:9000", Some(true));
        assert!(config
            .validate()
            .unwrap_err()
            .to_string()
            .contains("backpressure requires a TCP input"));
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
    fn supplied_production_config_converts_to_valid_rust_yaml() {
        let config: Config =
            serde_yaml::from_str(include_str!("../config.production.example.yaml")).unwrap();
        let valid = config.validate().unwrap();
        assert_eq!(valid.source.services.len(), 12);
        assert_eq!(valid.source.components.len(), 12);
        assert_eq!(valid.subchannels.len(), 12);
        assert_eq!(
            valid.subchannels.iter().map(|sub| sub.size_cu).sum::<u16>(),
            552
        );
        assert_eq!(valid.source.ensemble.tist_offset_ms, 2000);
        assert_eq!(valid.source.output.tagpacket_alignment, 16);
    }

    #[tokio::test]
    async fn invalid_candidate_does_not_replace_active_config() {
        let initial = example().validate().unwrap();
        let shared = SharedConfig::new(initial.clone());
        let mut invalid = example();
        invalid.components.clear();
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
        layout_change.subchannels[0].bitrate = 104;
        assert!(shared.apply(layout_change).await.unwrap());
        assert_eq!(shared.read().await.subchannels[0].bitrate, 104);
    }
}
