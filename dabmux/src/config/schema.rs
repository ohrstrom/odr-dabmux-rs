//! Operator-facing configuration. Services own their components, and each
//! component normally defines its own anonymous subchannel; only subchannels
//! shared by several components are named, in the top-level `subchannels` map.
//! [`Config::normalize`] turns this into the flat [`Multiplex`] the mux runs on.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{bail, Context};
use serde::Deserialize;

use super::{
    number, service_name, Component, EdiOutputConfig, EnsembleConfig, InputConfig, InputTiming,
    Multiplex, ProtectionConfig, Service, Subchannel, SubchannelKind, UserApplication,
    ValidatedConfig,
};

/// Protection used when neither the subchannel nor `defaults` sets one.
pub const DEFAULT_PROTECTION: ProtectionConfig = ProtectionConfig::EepA { level: 3 };
pub const DEFAULT_STREAM_INDEX: u16 = 1;
pub const DEFAULT_BUFFER_FRAMES: usize = 40;
pub const DEFAULT_PREBUFFER_FRAMES: usize = 4;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub ensemble: EnsembleConfig,
    #[serde(default)]
    pub defaults: Defaults,
    /// Subchannels shared by several components, by configuration-local name.
    #[serde(default)]
    pub subchannels: BTreeMap<String, SubchannelConfig>,
    pub services: Vec<ServiceConfig>,
    pub output: EdiOutputConfig,
}

/// Settings applied wherever a subchannel or input leaves them unset.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub protection: Option<ProtectionConfig>,
    #[serde(default)]
    pub edi: EdiDefaults,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EdiDefaults {
    pub buffer_frames: Option<usize>,
    pub prebuffer_frames: Option<usize>,
    pub timing: Option<InputTiming>,
    /// Applies to TCP inputs only; UDP inputs cannot be throttled.
    pub backpressure: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    #[serde(deserialize_with = "number::deserialize")]
    pub id: u32,
    pub label: String,
    pub short_label: Option<String>,
    #[serde(default, deserialize_with = "number::deserialize")]
    pub pty: u8,
    #[serde(default, deserialize_with = "number::deserialize")]
    pub language: u8,
    /// In service order: the first is the primary component.
    pub components: Vec<ComponentConfig>,
}

/// A component either defines its own subchannel (`type`, `bitrate`, `input`
/// and optionally `protection` and `subchannel_id`) or references a shared
/// one by name, never both.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ComponentConfig {
    pub subchannel: Option<String>,
    #[serde(default, deserialize_with = "number::option")]
    pub subchannel_id: Option<u8>,
    #[serde(rename = "type")]
    pub kind: Option<SubchannelKind>,
    pub bitrate: Option<u16>,
    pub protection: Option<ProtectionConfig>,
    pub input: Option<InputSpec>,
    #[serde(default)]
    pub user_applications: Vec<UserApplication>,
}

/// A shared subchannel. `id` is the transmitted SubChId, allocated when unset.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubchannelConfig {
    #[serde(default, deserialize_with = "number::option")]
    pub id: Option<u8>,
    #[serde(rename = "type")]
    pub kind: SubchannelKind,
    pub bitrate: u16,
    pub protection: Option<ProtectionConfig>,
    pub input: InputSpec,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "protocol", deny_unknown_fields)]
pub enum InputSpec {
    Edi {
        uri: String,
        stream_index: Option<u16>,
        buffer_frames: Option<usize>,
        prebuffer_frames: Option<usize>,
        timing: Option<InputTiming>,
        /// TCP only, on by default: stop reading while the buffer is full so
        /// the producer is throttled, which unpaced file encoders need. Set
        /// `false` for live encoders to drop the oldest frame instead, so that
        /// clock drift cannot grow latency without bound.
        backpressure: Option<bool>,
    },
    Sti {
        uri: String,
    },
}

impl ComponentConfig {
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
                                    kind: shared.kind.clone(),
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
                            kind: kind.clone(),
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
                components.push(Component {
                    service: index,
                    subchannel,
                    user_applications: component.user_applications.clone(),
                });
            }
            services.push(Service {
                id: service.id,
                label: service.label.clone(),
                short_label: service.short_label.clone(),
                pty: service.pty,
                language: service.language,
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
        }
    }
}
