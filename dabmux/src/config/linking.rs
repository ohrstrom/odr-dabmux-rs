//! Service following: service linking (FIG 0/6), OE services (FIG 0/24),
//! frequency information (FIG 0/21) and service component information for
//! announced changes (FIG 0/20), per EN 300 401 and TS 103 176.
//!
//! The operator types here are used as they are in the internal model;
//! [`Databases::build`] validates them and derives what the standards define
//! from content, such as the ILS, OE and P/D flags and the identifier order.

use std::collections::HashSet;

use anyhow::{bail, Context};
use chrono::{DateTime, Timelike, Utc};
use serde::{Deserialize, Serialize};

use super::number::{self, Frequency};
use super::{service_name, short_label_mask, validate_label, Multiplex, ValidatedService};

/// A linkage set with the service it is configured on as the key service.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LinkageSet {
    /// Linkage Set Number, 1..=0xFFF, coordinated nationally.
    #[serde(deserialize_with = "number::deserialize")]
    pub lsn: u16,
    /// Hard: same programme; soft: related programmes.
    #[serde(default = "default_true")]
    pub hard: bool,
    /// Linkage actuator: receivers may follow the links only while active.
    #[serde(default = "default_true")]
    pub active: bool,
    /// International linkage set (ILS). Derived when unset: required for DRM
    /// or AMSS links and for identifiers with another country's ECC.
    pub international: Option<bool>,
    /// Alternative sources besides the key service. An empty list in a hard
    /// set is a "dead link" that stops service following to FM.
    #[serde(default)]
    pub links: Vec<Link>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Link {
    #[serde(rename = "type")]
    pub kind: LinkKind,
    /// DAB SId (16 or 32 bits), RDS PI code, or the 24-bit DRM/AMSS SId.
    #[serde(deserialize_with = "number::deserialize")]
    pub id: u32,
    /// ECC of a DAB or FM identifier; defaults to the ensemble ECC.
    #[serde(default, deserialize_with = "number::option")]
    pub ecc: Option<u8>,
    #[serde(default)]
    pub preference: Preference,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Dab,
    Fm,
    Drm,
    Amss,
}

/// IdLP: identifiers of low preference are offered after the others.
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    #[default]
    Normal,
    Low,
}

/// A service not carried in this ensemble, announced in other ensembles.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OtherService {
    /// 16-bit programme or 32-bit data service ID.
    #[serde(deserialize_with = "number::deserialize")]
    pub id: u32,
    #[serde(deserialize_with = "number::list")]
    pub ensembles: Vec<u16>,
}

/// Frequency information for an ensemble or another bearer.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "type", deny_unknown_fields)]
pub enum FrequencyInformation {
    Dab {
        #[serde(deserialize_with = "number::deserialize")]
        eid: u16,
        /// DAB: frequencies carry co-timed, synchronised signals.
        #[serde(default)]
        continuity: bool,
        frequencies: Vec<DabFrequency>,
    },
    Fm {
        #[serde(deserialize_with = "number::deserialize")]
        pi: u16,
        /// The audio is delayed to match DAB decoding (tuned services only).
        #[serde(default)]
        continuity: bool,
        other_ensemble: Option<bool>,
        frequencies: Vec<Frequency>,
    },
    Drm {
        /// 24-bit DRM service identifier.
        #[serde(deserialize_with = "number::deserialize")]
        id: u32,
        #[serde(default)]
        continuity: bool,
        other_ensemble: Option<bool>,
        frequencies: Vec<Frequency>,
    },
    Amss {
        /// 24-bit AMSS service identifier.
        #[serde(deserialize_with = "number::deserialize")]
        id: u32,
        #[serde(default)]
        continuity: bool,
        other_ensemble: Option<bool>,
        frequencies: Vec<Frequency>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DabFrequency {
    pub mhz: Frequency,
    /// The other transmitter serves a geographically adjacent area.
    #[serde(default)]
    pub adjacent: bool,
    /// The ensemble there uses transmission mode I.
    #[serde(default)]
    pub mode_i: bool,
}

/// Advance information about a change to a service element (FIG 0/20).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServiceChange {
    #[serde(deserialize_with = "number::deserialize")]
    pub id: u32,
    /// Component within the service; 0, the default, means the whole service.
    #[serde(default)]
    pub scids: u8,
    pub change: ChangeKind,
    #[serde(default)]
    pub part_time: bool,
    /// When the change happens, in UTC, at most 28 days ahead.
    pub at: Option<DateTime<Utc>>,
    /// Service to select instead (identity changes and removals).
    #[serde(default, deserialize_with = "number::option")]
    pub transfer_sid: Option<u32>,
    /// Ensemble carrying the transfer service, if not this one.
    #[serde(default, deserialize_with = "number::option")]
    pub transfer_eid: Option<u16>,
    /// Component type: ASCTy for audio, or DSCTy for data.
    #[serde(default, deserialize_with = "number::option")]
    pub ascty: Option<u8>,
    #[serde(default, deserialize_with = "number::option")]
    pub dscty: Option<u8>,
    #[serde(default)]
    pub access_controlled: bool,
    /// Label of a service not (yet) in this ensemble, signalled with it.
    pub label: Option<String>,
    pub short_label: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// New SId, or moved to or from another ensemble.
    Identity,
    Addition,
    /// Removed from this ensemble only.
    LocalRemoval,
    /// Removed from all ensembles.
    GlobalRemoval,
}

/// One identifier of a linkage set, in transmission order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkId {
    /// Identifier list qualifier: 0 DAB, 1 RDS, 3 DRM/AMSS.
    pub idlq: u8,
    pub low_preference: bool,
    pub id: u32,
    /// ECC byte for ILS = 1 (programme services).
    pub ecc: u8,
}

/// A validated linkage set, ready to encode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkageEntry {
    /// Index of the key service.
    pub service: usize,
    pub lsn: u16,
    pub hard: bool,
    pub active: bool,
    pub international: bool,
    /// P/D: the key service is a data service.
    pub data: bool,
    /// Key service first, then TS 103 176 transmission order.
    pub ids: Vec<LinkId>,
}

impl LinkageEntry {
    /// Database key: P/D, S/H, ILS and LSN.
    pub fn key(&self) -> (bool, bool, bool, u16) {
        (self.data, self.hard, self.international, self.lsn)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OeEntry {
    pub other_ensemble: bool,
    pub data: bool,
    pub sid: u32,
    pub eids: Vec<u16>,
}

/// FI list with its frequency list already encoded, one item per frequency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiEntry {
    pub other_ensemble: bool,
    /// Range and modulation: 0 DAB, 6 DRM, 8 FM, 14 AMSS.
    pub range_modulation: u8,
    pub id: u16,
    /// Id field 2 (DRM and AMSS: top byte of the 24-bit SId).
    pub id2: Option<u8>,
    pub continuity: bool,
    pub frequencies: Vec<Vec<u8>>,
}

impl FiEntry {
    /// Database key: OE, Id field and R&M.
    pub fn key(&self) -> (bool, u16, u8) {
        (self.other_ensemble, self.id, self.range_modulation)
    }

    /// Frequencies per FI list: the 3-bit length field allows 7 bytes.
    pub fn per_list(&self) -> usize {
        let item = self.frequencies.first().map_or(1, Vec::len);
        (7 - usize::from(self.id2.is_some())) / item
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SciEntry {
    pub data: bool,
    pub sid: u32,
    pub scids: u8,
    pub change: ChangeKind,
    pub part_time: bool,
    /// CA flag, A/D flag and SCTy.
    pub description: Option<(bool, bool, u8)>,
    /// Date (MJD, low 5 bits), hour, minute, second, or the special value.
    pub date_time: (u8, u8, u8, u8),
    pub transfer_sid: Option<u32>,
    pub transfer_eid: Option<u16>,
}

/// A label for a service outside the ensemble, sent with its FIG 0/20.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncedLabel {
    pub data: bool,
    pub sid: u32,
    pub label: String,
    pub short_label: Option<String>,
}

/// Everything service following signals, validated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Databases {
    pub linkage: Vec<LinkageEntry>,
    pub other_ensembles: Vec<OeEntry>,
    pub frequencies: Vec<FiEntry>,
    pub changes: Vec<SciEntry>,
    pub labels: Vec<AnnouncedLabel>,
}

impl Databases {
    pub fn build(multiplex: &Multiplex, services: &[ValidatedService]) -> anyhow::Result<Self> {
        let ensemble = &multiplex.ensemble;
        let carried: HashSet<u32> = multiplex.services.iter().map(|s| s.id).collect();
        let data_of = |id: u32| -> bool {
            multiplex
                .services
                .iter()
                .position(|s| s.id == id)
                .map_or(id > u32::from(u16::MAX), |index| services[index].data)
        };
        let mut databases = Databases::default();

        let mut keys = HashSet::new();
        for (index, service) in multiplex.services.iter().enumerate() {
            let name = service_name(service.id);
            let data = services[index].data;
            let mut active = (0, 0);
            for set in &service.linking {
                let path = format!("service {name} linking lsn 0x{:03X}", set.lsn);
                let entry = linkage_entry(
                    index,
                    service.id,
                    service.ecc,
                    data,
                    set,
                    ensemble.ecc,
                    &carried,
                )
                .with_context(|| path.clone())?;
                if !keys.insert(entry.key()) {
                    bail!("{path}: another linkage set has the same LSN, hard and international flags");
                }
                if entry.active {
                    if entry.hard {
                        active.0 += 1;
                    } else {
                        active.1 += 1;
                    }
                }
                databases.linkage.push(entry);
            }
            if active.0 > 1 || active.1 > 1 {
                bail!("service {name}: at most one hard and one soft linkage set may be active");
            }
            if !service.other_ensembles.is_empty() {
                databases.other_ensembles.push(
                    oe_entry(false, data, service.id, &service.other_ensembles)
                        .with_context(|| format!("service {name} other_ensembles"))?,
                );
            }
        }

        let mut foreign = HashSet::new();
        for other in &multiplex.other_services {
            let name = service_name(other.id);
            if carried.contains(&other.id) {
                bail!("other_services {name} is carried here; set other_ensembles on the service");
            }
            if !foreign.insert(other.id) {
                bail!("other_services lists {name} twice");
            }
            databases.other_ensembles.push(
                oe_entry(
                    true,
                    other.id > u32::from(u16::MAX),
                    other.id,
                    &other.ensembles,
                )
                .with_context(|| format!("other_services {name}"))?,
            );
        }
        // OE = 0 entries first saves FIG headers, as in ODR-DabMux.
        databases
            .other_ensembles
            .sort_by_key(|entry| entry.other_ensemble);

        // FM, DRM and AMSS sources of services in this ensemble have OE = 0:
        // the SIds themselves (implicit FM linking) and every linked id.
        let mut tuned_ids: HashSet<(u8, u32)> = carried.iter().map(|&id| (1, id)).collect();
        for entry in &databases.linkage {
            for id in &entry.ids[1..] {
                tuned_ids.insert((id.idlq, id.id));
            }
        }
        let mut fi_keys = HashSet::new();
        for (position, fi) in multiplex.frequencies.iter().enumerate() {
            let path = format!("frequencies[{position}]");
            let entry = fi_entry(fi, ensemble.id, &tuned_ids).with_context(|| path.clone())?;
            if !fi_keys.insert(entry.key()) {
                bail!("{path} repeats the frequency information of an earlier entry");
            }
            databases.frequencies.push(entry);
        }
        databases
            .frequencies
            .sort_by_key(|entry| entry.other_ensemble);

        for (position, change) in multiplex.service_changes.iter().enumerate() {
            let path = format!("service_changes[{position}] ({})", service_name(change.id));
            let data = data_of(change.id);
            databases
                .changes
                .push(sci_entry(change, data).with_context(|| path.clone())?);
            match (&change.label, carried.contains(&change.id)) {
                (Some(_), true) => {
                    bail!("{path}: the label of a carried service comes from the service")
                }
                (Some(label), false) => {
                    validate_label(&format!("{path} label"), label)?;
                    short_label_mask(
                        &format!("{path} short_label"),
                        label,
                        change.short_label.as_deref(),
                    )?;
                    if !databases.labels.iter().any(|l| l.sid == change.id) {
                        databases.labels.push(AnnouncedLabel {
                            data,
                            sid: change.id,
                            label: label.clone(),
                            short_label: change.short_label.clone(),
                        });
                    }
                }
                (None, _) if change.short_label.is_some() => {
                    bail!("{path}: short_label needs a label")
                }
                (None, _) => {}
            }
        }
        Ok(databases)
    }
}

fn linkage_entry(
    service: usize,
    sid: u32,
    service_ecc: Option<u8>,
    data: bool,
    set: &LinkageSet,
    ensemble_ecc: u8,
    carried: &HashSet<u32>,
) -> anyhow::Result<LinkageEntry> {
    if set.lsn == 0 || set.lsn > 0x0fff {
        bail!("lsn must be 1..=0xFFF");
    }
    if set.links.is_empty() && (!set.hard || data) {
        bail!("only a hard linkage set of a programme service may have no links (a dead link)");
    }
    if set.links.len() + 1 > 128 {
        bail!("a linkage set holds at most 128 identifiers");
    }
    let mut needs_ils = !data && service_ecc.is_some_and(|ecc| ecc != ensemble_ecc);
    let mut seen = HashSet::new();
    for link in &set.links {
        let id_bits = match (data, link.kind) {
            (false, LinkKind::Dab | LinkKind::Fm) => 16,
            (_, LinkKind::Drm | LinkKind::Amss) => 24,
            (true, LinkKind::Dab) => 32,
            (true, LinkKind::Fm) => bail!("data services link to DAB and DRM services only"),
        };
        if data && link.kind == LinkKind::Amss {
            bail!("data services link to DAB and DRM services only");
        }
        if id_bits < 32 && link.id >> id_bits != 0 {
            bail!("{:?} id 0x{:X} exceeds {id_bits} bits", link.kind, link.id);
        }
        if link.id == 0 {
            bail!("link ids must not be 0");
        }
        if link.ecc.is_some() && (data || matches!(link.kind, LinkKind::Drm | LinkKind::Amss)) {
            bail!("ecc applies to DAB and FM links of programme services only");
        }
        if link.kind == LinkKind::Dab && link.id == sid {
            bail!("the key service is linked implicitly; do not list it");
        }
        if !seen.insert((link.kind, link.id, link.ecc)) {
            bail!("{:?} 0x{:X} is listed twice", link.kind, link.id);
        }
        needs_ils |= !data
            && (matches!(link.kind, LinkKind::Drm | LinkKind::Amss)
                || link.ecc.is_some_and(|ecc| ecc != ensemble_ecc));
    }
    let international = match set.international {
        Some(false) if needs_ils => {
            bail!("international must be true: the set has DRM/AMSS links or another country's ECC")
        }
        Some(international) => international,
        None => needs_ils,
    };

    // TS 103 176 clause 5.2.3 order: DAB services carried here, other DAB,
    // RDS, then DRM/AMSS; normal preference before low within each group.
    let rank = |link: &Link| -> u8 {
        match link.kind {
            LinkKind::Dab if carried.contains(&link.id) => 0,
            LinkKind::Dab => 1,
            LinkKind::Fm => 2,
            LinkKind::Drm | LinkKind::Amss => 3,
        }
    };
    let mut ordered: Vec<&Link> = set.links.iter().collect();
    ordered.sort_by_key(|link| (rank(link), link.preference == Preference::Low));
    let mut ids = vec![LinkId {
        idlq: 0,
        low_preference: false,
        id: sid,
        ecc: service_ecc.unwrap_or(ensemble_ecc),
    }];
    ids.extend(ordered.into_iter().map(|link| {
        let (idlq, id, ecc) = match link.kind {
            LinkKind::Dab => (0, link.id, link.ecc.unwrap_or(ensemble_ecc)),
            LinkKind::Fm => (1, link.id, link.ecc.unwrap_or(ensemble_ecc)),
            // ECC field carries the top byte of the 24-bit identifier.
            LinkKind::Drm | LinkKind::Amss => (3, link.id, (link.id >> 16) as u8),
        };
        LinkId {
            idlq,
            low_preference: link.preference == Preference::Low,
            id,
            ecc,
        }
    }));
    Ok(LinkageEntry {
        service,
        lsn: set.lsn,
        hard: set.hard,
        active: set.active,
        international,
        data,
        ids,
    })
}

fn oe_entry(other_ensemble: bool, data: bool, sid: u32, eids: &[u16]) -> anyhow::Result<OeEntry> {
    if eids.is_empty() {
        bail!("needs at least one ensemble id");
    }
    let mut seen = HashSet::new();
    if let Some(eid) = eids.iter().find(|eid| !seen.insert(**eid)) {
        bail!("lists ensemble 0x{eid:04X} twice");
    }
    Ok(OeEntry {
        other_ensemble,
        data,
        sid,
        eids: eids.to_vec(),
    })
}

fn fi_entry(
    fi: &FrequencyInformation,
    ensemble_id: u16,
    tuned_ids: &HashSet<(u8, u32)>,
) -> anyhow::Result<FiEntry> {
    let derive_oe = |explicit: Option<bool>, idlq: u8, id: u32| {
        explicit.unwrap_or(!tuned_ids.contains(&(idlq, id)))
    };
    let check_continuity = |oe: bool, continuity: bool| {
        if oe && continuity {
            bail!("continuity applies to services of this ensemble only (other_ensemble is set)");
        }
        Ok(())
    };
    let entry = match fi {
        FrequencyInformation::Dab {
            eid,
            continuity,
            frequencies,
        } => FiEntry {
            other_ensemble: *eid != ensemble_id,
            range_modulation: 0,
            id: *eid,
            id2: None,
            continuity: *continuity,
            frequencies: frequencies
                .iter()
                .map(|f| {
                    if f.mhz.khz % 16 != 0 || f.mhz.khz / 16 >= 1 << 19 {
                        bail!(
                            "DAB frequency {} kHz is not a multiple of 16 kHz",
                            f.mhz.khz
                        );
                    }
                    let control = u32::from(!f.adjacent) | (u32::from(f.mode_i) << 1);
                    let value = (control << 19) | (f.mhz.khz / 16);
                    Ok(value.to_be_bytes()[1..].to_vec())
                })
                .collect::<anyhow::Result<_>>()?,
        },
        FrequencyInformation::Fm {
            pi,
            continuity,
            other_ensemble,
            frequencies,
        } => {
            let oe = derive_oe(*other_ensemble, 1, u32::from(*pi));
            check_continuity(oe, *continuity)?;
            FiEntry {
                other_ensemble: oe,
                range_modulation: 8,
                id: *pi,
                id2: None,
                continuity: *continuity,
                frequencies: frequencies
                    .iter()
                    .map(|f| {
                        let steps = f
                            .khz
                            .checked_sub(87_500)
                            .filter(|s| s % 100 == 0)
                            .map(|s| s / 100);
                        match steps {
                            Some(code @ 1..=204) => Ok(vec![code as u8]),
                            _ => bail!(
                                "FM frequency {} kHz is not 87.6..=107.9 MHz in 100 kHz steps",
                                f.khz
                            ),
                        }
                    })
                    .collect::<anyhow::Result<_>>()?,
            }
        }
        FrequencyInformation::Drm {
            id,
            continuity,
            other_ensemble,
            frequencies,
        }
        | FrequencyInformation::Amss {
            id,
            continuity,
            other_ensemble,
            frequencies,
        } => {
            let drm = matches!(fi, FrequencyInformation::Drm { .. });
            if *id >> 24 != 0 || *id == 0 {
                bail!("id must be a nonzero 24-bit service identifier");
            }
            let oe = derive_oe(*other_ensemble, 3, *id);
            check_continuity(oe, *continuity)?;
            FiEntry {
                other_ensemble: oe,
                range_modulation: if drm { 6 } else { 14 },
                id: *id as u16,
                id2: Some((*id >> 16) as u8),
                continuity: *continuity,
                frequencies: frequencies
                    .iter()
                    .map(|f| {
                        // DRM above 32.767 MHz is robustness mode E, in 10 kHz units.
                        let field = match (drm, f.khz) {
                            (_, khz @ 1..=32_767) => khz,
                            (true, khz) if khz % 10 == 0 && khz / 10 <= 32_767 => {
                                0x8000 | (khz / 10)
                            }
                            (_, khz) => bail!("frequency {khz} kHz is out of range"),
                        };
                        Ok((field as u16).to_be_bytes().to_vec())
                    })
                    .collect::<anyhow::Result<_>>()?,
            }
        }
    };
    if entry.frequencies.is_empty() {
        bail!("needs at least one frequency");
    }
    Ok(entry)
}

fn sci_entry(change: &ServiceChange, data: bool) -> anyhow::Result<SciEntry> {
    if change.scids > 15 {
        bail!("scids must be 0..=15");
    }
    let description = match (change.ascty, change.dscty) {
        (Some(_), Some(_)) => bail!("set ascty or dscty, not both"),
        (Some(ascty), None) => Some((change.access_controlled, false, ascty)),
        (None, Some(dscty)) => Some((change.access_controlled, true, dscty)),
        (None, None) if change.access_controlled => bail!("access_controlled needs ascty or dscty"),
        (None, None) => None,
    };
    if description.is_some_and(|(_, _, scty)| scty > 63) {
        bail!("ascty and dscty must be 0..=63");
    }
    if let Some(transfer) = change.transfer_sid {
        if !data && transfer > u32::from(u16::MAX) {
            bail!("transfer_sid of a programme service must fit 16 bits");
        }
    }
    let date_time = match change.at {
        // Hour, minute and second all ones: no time signalled.
        None => (0x1f, 0x1f, 0x3f, 0x3f),
        Some(at) => {
            let mjd = at.timestamp().div_euclid(86_400) + 40_587;
            (
                (mjd & 0x1f) as u8,
                at.hour() as u8,
                at.minute() as u8,
                at.second() as u8,
            )
        }
    };
    Ok(SciEntry {
        data,
        sid: change.id,
        scids: change.scids,
        change: change.change,
        part_time: change.part_time,
        description,
        date_time,
        transfer_sid: change.transfer_sid,
        transfer_eid: change.transfer_eid,
    })
}
