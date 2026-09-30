use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

pub mod reload;
pub mod watch;

#[derive(Clone)]
pub struct SharedConfig(Arc<RwLock<ValidatedConfig>>);

impl SharedConfig {
    pub fn new(config: ValidatedConfig) -> Self {
        Self(Arc::new(RwLock::new(config)))
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, ValidatedConfig> {
        self.0.read().await
    }

    async fn replace_if_changed(&self, config: ValidatedConfig) -> anyhow::Result<bool> {
        let mut current = self.0.write().await;
        if *current == config {
            return Ok(false);
        }
        let old = &current.source;
        let new = &config.source;
        if old.ensemble.id != new.ensemble.id
            || old.ensemble.ecc != new.ensemble.ecc
            || old.ensemble.mode != new.ensemble.mode
            || old.ensemble.local_time_offset_half_hours
                != new.ensemble.local_time_offset_half_hours
            || old.ensemble.international_table != new.ensemble.international_table
            || old.ensemble.reconfiguration_counter != new.ensemble.reconfiguration_counter
            || old.ensemble.tist != new.ensemble.tist
            || old.ensemble.tai_utc_offset != new.ensemble.tai_utc_offset
            || old.subchannels != new.subchannels
            || old.components != new.components
            || old.output != new.output
            || old
                .services
                .iter()
                .map(|s| (&s.uid, s.id))
                .collect::<Vec<_>>()
                != new
                    .services
                    .iter()
                    .map(|s| (&s.uid, s.id))
                    .collect::<Vec<_>>()
        {
            bail!("this config change requires a mux restart; only labels can reload live");
        }
        *current = config;
        Ok(true)
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
    #[serde(default)]
    pub local_time_offset_half_hours: i8,
    #[serde(default)]
    pub international_table: u8,
    pub reconfiguration_counter: Option<u16>,
    #[serde(default = "default_mode")]
    pub mode: u8,
    #[serde(default)]
    pub tist: bool,
    pub tai_utc_offset: Option<u8>,
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
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentConfig {
    pub uid: String,
    pub service: String,
    pub subchannel: String,
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
    },
    Sti {
        uri: String,
    },
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
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "protocol", deny_unknown_fields)]
pub enum EdiDestination {
    Udp { address: String, port: u16 },
    Tcp { listen_port: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedConfig {
    pub source: Config,
    pub subchannels: Vec<ValidatedSubchannel>,
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
        if let Some(counter) = self.ensemble.reconfiguration_counter {
            if counter > 1023 || self.services.len() > 63 {
                bail!("invalid reconfiguration counter or service count");
            }
        }
        if self.ensemble.tist && !matches!(self.ensemble.tai_utc_offset, Some(32..=255)) {
            bail!("ensemble.tist requires tai_utc_offset >= 32");
        }
        validate_label("ensemble.label", &self.ensemble.label)?;
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

        let mut service_uids = HashSet::new();
        let mut service_ids = HashSet::new();
        for service in &self.services {
            validate_uid("service", &service.uid)?;
            validate_label("service.label", &service.label)?;
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
            let uri = match &sub.input {
                InputConfig::Edi { uri, .. } | InputConfig::Sti { uri } => uri,
            };
            let address = match &sub.input {
                InputConfig::Edi { .. } => uri
                    .strip_prefix("udp://")
                    .or_else(|| uri.strip_prefix("tcp://")),
                InputConfig::Sti { .. } => uri.strip_prefix("rtp://"),
            }
            .ok_or_else(|| {
                anyhow::anyhow!("subchannel {} input URI has unsupported transport", sub.uid)
            })?;
            let address = if address.starts_with(':') {
                format!("0.0.0.0{address}")
            } else {
                address.to_owned()
            };
            address.parse::<std::net::SocketAddr>().with_context(|| {
                format!(
                    "subchannel {} input URI needs an IP address and port",
                    sub.uid
                )
            })?;
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
            });
            next_cu = end;
        }

        let mut component_uids = HashSet::new();
        let mut linked_services = HashSet::new();
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
            linked_services.insert(component.service.as_str());
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
        for destination in &self.output.destinations {
            match destination {
                EdiDestination::Udp { address, port } => {
                    if *port == 0 || address.parse::<std::net::IpAddr>().is_err() {
                        bail!("EDI UDP destination needs an IP address and nonzero port");
                    }
                }
                EdiDestination::Tcp { listen_port } if *listen_port == 0 => {
                    bail!("EDI TCP listen_port must be nonzero")
                }
                EdiDestination::Tcp { .. } => {}
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

fn validate_label(path: &str, label: &str) -> anyhow::Result<()> {
    if label.is_empty()
        || label.len() > 16
        || !label.is_ascii()
        || label.chars().any(|c| c.is_ascii_control())
    {
        bail!("{path} must contain 1..=16 printable ASCII characters");
    }
    Ok(())
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
        serde_yaml::from_str(include_str!("../config.example.yaml")).unwrap()
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

    #[tokio::test]
    async fn invalid_candidate_does_not_replace_active_config() {
        let initial = example().validate().unwrap();
        let shared = SharedConfig::new(initial.clone());
        let mut invalid = example();
        invalid.components.clear();
        assert!(invalid.validate().is_err());
        assert_eq!(*shared.read().await, initial);

        let mut changed = example();
        changed.services[0].label = "New Label".into();
        assert!(shared.apply(changed).await.unwrap());
        assert_eq!(shared.read().await.source.services[0].label, "New Label");
        let mut layout_change = example();
        layout_change.subchannels[0].bitrate = 104;
        assert!(shared
            .apply(layout_change)
            .await
            .unwrap_err()
            .to_string()
            .contains("restart"));
    }
}
