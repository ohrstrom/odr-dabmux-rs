//! Operator-facing configuration. Services own their components, and each
//! component normally defines its own anonymous subchannel; only subchannels
//! shared by several components are named, in the top-level `subchannels` map.
//! [`Config::normalize`] turns this into the flat [`Multiplex`] the mux runs on.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use super::{
    number, service_name, Component, EdiOutputConfig, EnsembleConfig, InputConfig, InputTiming,
    Multiplex, PacketComponent, ProtectionConfig, Service, Subchannel, SubchannelKind,
    UserApplication, ValidatedConfig,
};
use crate::config::linking::{FrequencyInformation, LinkageSet, OtherService, ServiceChange};

/// Protection used when neither the subchannel nor `defaults` sets one.
pub const DEFAULT_PROTECTION: ProtectionConfig = ProtectionConfig::EepA { level: 3 };
pub const DEFAULT_STREAM_INDEX: u16 = 1;
pub const DEFAULT_BUFFER_FRAMES: usize = 40;
pub const DEFAULT_PREBUFFER_FRAMES: usize = 4;
/// Data service component type MOT (TS 101 756 table 2b), used by SPI and slideshow.
pub const DSCTY_MOT: u8 = 60;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub ensemble: EnsembleConfig,
    #[serde(default)]
    #[serde(skip_serializing_if = "Defaults::is_empty")]
    pub defaults: Defaults,
    /// Subchannels shared by several components, by configuration-local name.
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub subchannels: BTreeMap<String, SubchannelConfig>,
    pub services: Vec<ServiceConfig>,
    /// Services carried only in other ensembles (FIG 0/24 with OE = 1).
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_services: Vec<OtherService>,
    /// Frequency information (FIG 0/21).
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub frequencies: Vec<FrequencyInformation>,
    /// Advance information about service changes (FIG 0/20).
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub service_changes: Vec<ServiceChange>,
    pub output: EdiOutputConfig,
}

/// Settings applied wherever a subchannel or input leaves them unset.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protection: Option<ProtectionConfig>,
    #[serde(default)]
    #[serde(skip_serializing_if = "EdiDefaults::is_empty")]
    pub edi: EdiDefaults,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EdiDefaults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_frames: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prebuffer_frames: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timing: Option<InputTiming>,
    /// Applies to TCP inputs only; UDP inputs cannot be throttled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backpressure: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    /// 16 bits for programme services; data services use the 32-bit form
    /// with the ECC in its top byte.
    #[serde(deserialize_with = "number::deserialize")]
    pub id: u32,
    /// ECC of a programme service from another country (FIG 0/9 extended field).
    #[serde(default, deserialize_with = "number::option")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecc: Option<u8>,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_label: Option<String>,
    #[serde(default, deserialize_with = "number::deserialize")]
    pub pty: u8,
    #[serde(default, deserialize_with = "number::deserialize")]
    pub language: u8,
    /// In service order: the first is the primary component.
    pub components: Vec<ComponentConfig>,
    /// Linkage sets with this service as the key service (FIG 0/6).
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linking: Vec<LinkageSet>,
    /// Other ensembles that also carry this service (FIG 0/24).
    #[serde(default, deserialize_with = "number::list")]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_ensembles: Vec<u16>,
}

/// A component either defines its own subchannel (`type`, `bitrate`, `input`
/// and optionally `protection` and `subchannel_id`) or references a shared
/// one by name, never both.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subchannel: Option<String>,
    #[serde(default, deserialize_with = "number::option")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subchannel_id: Option<u8>,
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<SubchannelKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protection: Option<ProtectionConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<InputSpec>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub user_applications: Vec<UserApplication>,
    /// Packet mode only: the address of this component's packets, which must
    /// match the packets in the input.
    #[serde(default, deserialize_with = "number::option")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_address: Option<u16>,
    /// Packet mode only: data service component type; MOT (60) for `spi`
    /// and `slideshow`.
    #[serde(default, deserialize_with = "number::option")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dscty: Option<u8>,
    /// Packet mode only: whether MSC data groups are used (default true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_groups: Option<bool>,
}

/// A shared subchannel. `id` is the transmitted SubChId, allocated when unset.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubchannelConfig {
    #[serde(default, deserialize_with = "number::option")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u8>,
    #[serde(rename = "type")]
    pub kind: SubchannelKind,
    pub bitrate: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protection: Option<ProtectionConfig>,
    pub input: InputSpec,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "protocol", deny_unknown_fields)]
pub enum InputSpec {
    Edi {
        uri: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        stream_index: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        buffer_frames: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prebuffer_frames: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timing: Option<InputTiming>,
        /// TCP only, on by default: stop reading while the buffer is full so
        /// the producer is throttled, which unpaced file encoders need. Set
        /// `false` for live encoders to drop the oldest frame instead, so that
        /// clock drift cannot grow latency without bound.
        #[serde(skip_serializing_if = "Option::is_none")]
        backpressure: Option<bool>,
    },
    Sti {
        uri: String,
    },
    /// Ready-made packets for an `enhanced_packet` subchannel, read whole and
    /// repeated; a changed file takes over when the current one wraps.
    File {
        path: PathBuf,
    },
}

impl Defaults {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl EdiDefaults {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl ComponentConfig {
    /// Packet settings with defaults, for a component on a `kind` subchannel.
    fn packet(&self, kind: &SubchannelKind) -> anyhow::Result<Option<PacketComponent>> {
        if *kind != SubchannelKind::EnhancedPacket {
            if self.packet_address.is_some() || self.dscty.is_some() || self.data_groups.is_some() {
                bail!("packet_address, dscty and data_groups apply to packet mode components only");
            }
            return Ok(None);
        }
        let address = self
            .packet_address
            .context("packet mode components need packet_address")?;
        let mot = self
            .user_applications
            .iter()
            .any(|app| matches!(app, UserApplication::Spi | UserApplication::Slideshow));
        let dscty = match self.dscty {
            Some(dscty) => dscty,
            None if mot => DSCTY_MOT,
            None => bail!("packet mode components without a user application need dscty"),
        };
        Ok(Some(PacketComponent {
            address,
            dscty,
            data_groups: self.data_groups.unwrap_or(true),
        }))
    }

    fn defines_subchannel(&self) -> bool {
        self.subchannel_id.is_some()
            || self.kind.is_some()
            || self.bitrate.is_some()
            || self.protection.is_some()
            || self.input.is_some()
    }
}

/// A subchannel in order of first use, before SubChIds are allocated.
struct Draft {
    name: String,
    id: Option<u8>,
    kind: SubchannelKind,
    bitrate: u16,
    protection: ProtectionConfig,
    input: InputConfig,
}

impl Config {
    pub fn validate(self) -> anyhow::Result<ValidatedConfig> {
        self.normalize()?.validate()
    }

    /// Resolve references and defaults and allocate SubChIds.
    ///
    /// Subchannels are ordered by first use in service and component order,
    /// which also fixes their CU start addresses. Explicit SubChIds are
    /// reserved first; the rest take the lowest free ID in that order, so
    /// inserting a service renumbers the automatic IDs after it.
    pub fn normalize(self) -> anyhow::Result<Multiplex> {
        let protection = self
            .defaults
            .protection
            .clone()
            .unwrap_or(DEFAULT_PROTECTION);
        let mut drafts: Vec<Draft> = Vec::new();
        let mut named: HashMap<&str, usize> = HashMap::new();
        let mut services = Vec::with_capacity(self.services.len());
        let mut components = Vec::new();
        // Subchannel names derive from service IDs, so they must be unique first.
        let mut service_ids = HashSet::new();
        if let Some(duplicate) = self.services.iter().find(|s| !service_ids.insert(s.id)) {
            bail!("duplicate service id: {}", service_name(duplicate.id));
        }
        for (index, service) in self.services.iter().enumerate() {
            let service_path = format!("services[{index}] ({})", service_name(service.id));
            if service.components.is_empty() {
                bail!("{service_path} needs at least one component");
            }
            for (position, component) in service.components.iter().enumerate() {
                let path = format!("{service_path}.components[{position}]");
                let subchannel = match &component.subchannel {
                    Some(name) => {
                        if component.defines_subchannel() {
                            bail!(
                                "{path} references subchannel {name:?}; set its type, bitrate, \
                                 protection, input and id under subchannels.{name}"
                            );
                        }
                        match named.get(name.as_str()) {
                            Some(&existing) => existing,
                            None => {
                                let shared = self.subchannels.get(name).with_context(|| {
                                    format!("{path} refers to unknown subchannel {name:?}")
                                })?;
                                drafts.push(Draft {
                                    name: name.clone(),
                                    id: shared.id,
                                    kind: shared.kind,
                                    bitrate: shared.bitrate,
                                    protection: shared
                                        .protection
                                        .clone()
                                        .unwrap_or_else(|| protection.clone()),
                                    input: shared.input.resolve(&self.defaults.edi),
                                });
                                named.insert(name, drafts.len() - 1);
                                drafts.len() - 1
                            }
                        }
                    }
                    None => {
                        let (Some(kind), Some(bitrate), Some(input)) =
                            (&component.kind, component.bitrate, &component.input)
                        else {
                            bail!(
                                "{path} needs type, bitrate and input, or a reference to a \
                                 shared subchannel"
                            );
                        };
                        drafts.push(Draft {
                            name: format!("{}/{position}", service_name(service.id)),
                            id: component.subchannel_id,
                            kind: *kind,
                            bitrate,
                            protection: component
                                .protection
                                .clone()
                                .unwrap_or_else(|| protection.clone()),
                            input: input.resolve(&self.defaults.edi),
                        });
                        drafts.len() - 1
                    }
                };
                let packet = component
                    .packet(&drafts[subchannel].kind)
                    .with_context(|| path.clone())?;
                components.push(Component {
                    service: index,
                    subchannel,
                    user_applications: component.user_applications.clone(),
                    packet,
                });
            }
            services.push(Service {
                id: service.id,
                ecc: service.ecc,
                label: service.label.clone(),
                short_label: service.short_label.clone(),
                pty: service.pty,
                language: service.language,
                linking: service.linking.clone(),
                other_ensembles: service.other_ensembles.clone(),
            });
        }
        if let Some(unused) = self
            .subchannels
            .keys()
            .find(|name| !named.contains_key(name.as_str()))
        {
            bail!("subchannels.{unused} is not used by any component");
        }

        let mut reserved = HashMap::new();
        for draft in &drafts {
            if let Some(id) = draft.id {
                if id >= 64 {
                    bail!("subchannel {} id {id} must be 0..=63", draft.name);
                }
                if let Some(other) = reserved.insert(id, draft.name.as_str()) {
                    bail!(
                        "subchannel id {id} is set on both {other} and {}",
                        draft.name
                    );
                }
            }
        }
        let used: HashSet<u8> = reserved.keys().copied().collect();
        let mut free = (0..64u8).filter(|id| !used.contains(id));
        let subchannels = drafts
            .into_iter()
            .map(|draft| {
                let (id, allocated) = match draft.id {
                    Some(id) => (id, false),
                    None => (
                        free.next()
                            .context("at most 64 subchannels are supported")?,
                        true,
                    ),
                };
                Ok(Subchannel {
                    name: draft.name,
                    id,
                    allocated,
                    kind: draft.kind,
                    bitrate: draft.bitrate,
                    protection: draft.protection,
                    input: draft.input,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Multiplex {
            ensemble: self.ensemble,
            services,
            subchannels,
            components,
            other_services: self.other_services,
            frequencies: self.frequencies,
            service_changes: self.service_changes,
            output: self.output,
        })
    }
}

impl InputSpec {
    fn resolve(&self, defaults: &EdiDefaults) -> InputConfig {
        match self {
            Self::Edi {
                uri,
                stream_index,
                buffer_frames,
                prebuffer_frames,
                timing,
                backpressure,
            } => InputConfig::Edi {
                uri: uri.clone(),
                stream_index: stream_index.unwrap_or(DEFAULT_STREAM_INDEX),
                buffer_frames: buffer_frames
                    .or(defaults.buffer_frames)
                    .unwrap_or(DEFAULT_BUFFER_FRAMES),
                prebuffer_frames: prebuffer_frames
                    .or(defaults.prebuffer_frames)
                    .unwrap_or(DEFAULT_PREBUFFER_FRAMES),
                timing: timing.or(defaults.timing).unwrap_or_default(),
                backpressure: backpressure
                    .or(defaults.backpressure.filter(|_| uri.starts_with("tcp://"))),
            },
            Self::Sti { uri } => InputConfig::Sti { uri: uri.clone() },
            Self::File { path } => InputConfig::File { path: path.clone() },
        }
    }
}
