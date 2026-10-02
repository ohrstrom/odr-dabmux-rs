//! Encoding of the service following databases: FIG 0/6, 0/21 and 0/24 as
//! long-form definitions in a slow rotation, FIG 0/20 at once per second,
//! and the short forms that announce changes (TS 103 176 clause 5).

use std::collections::HashSet;

use super::Group;
use crate::config::linking::{ChangeKind, Databases, FiEntry, LinkageEntry, OeEntry, SciEntry};
use crate::config::ValidatedConfig;

/// Change indications repeat at least once per second for five seconds.
pub const EVENT_FRAMES: u64 = 5 * 1000 / 24 + 1;

const EXT_LINKING: u8 = 6;
const EXT_SCI: u8 = 20;
const EXT_FREQUENCIES: u8 = 21;
const EXT_OE_SERVICES: u8 = 24;

/// Second FIG header byte: C/N, OE, P/D and extension.
fn header(continuation: bool, other_ensemble: bool, data: bool, extension: u8) -> u8 {
    (u8::from(continuation) << 7)
        | (u8::from(other_ensemble) << 6)
        | (u8::from(data) << 5)
        | extension
}

fn sid_bytes(data: bool, sid: u32) -> Vec<u8> {
    if data {
        sid.to_be_bytes().to_vec()
    } else {
        (sid as u16).to_be_bytes().to_vec()
    }
}

/// Collect `(header, entry)` pairs into groups, keeping the first-seen
/// order of headers and of entries within each.
fn grouped(entries: impl IntoIterator<Item = (u8, Vec<u8>)>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for (extension, entry) in entries {
        match groups.iter_mut().find(|group| group.extension == extension) {
            Some(group) => group.entries.push(entry),
            None => groups.push(Group {
                extension,
                entries: vec![entry],
            }),
        }
    }
    // Starts of database (C/N = 0) before their continuations.
    groups.sort_by_key(|group| group.extension >> 7);
    groups
}

/// The two bytes of a FIG 0/6 service linking field before the Id list.
fn linking_flags(entry: &LinkageEntry, id_list: bool) -> [u8; 2] {
    [
        (u8::from(id_list) << 7)
            | (u8::from(entry.active) << 6)
            | (u8::from(entry.hard) << 5)
            | (u8::from(entry.international) << 4)
            | (entry.lsn >> 8) as u8,
        entry.lsn as u8,
    ]
}

/// Service linking fields of one linkage set per TS 103 176 clause 5.2.4.1:
/// a start of database field (step A), then continuations (step B).
pub fn linkage_fields(entry: &LinkageEntry) -> Vec<(bool, Vec<u8>)> {
    let max = if entry.data {
        6
    } else if entry.international {
        8
    } else {
        12
    };
    let ids = &entry.ids;
    let (key, rest) = (&ids[0], &ids[1..]);
    let count = |idlq| rest.iter().filter(|id| id.idlq == idlq).count();
    // Identifiers of the start field: the key, then a run of the same kind
    // and preference as the qualifier chosen for it.
    let (idlq, run) = if rest.is_empty() {
        (1, 0) // dead link: the key alone, qualified as RDS
    } else if rest[0].low_preference != key.low_preference {
        (0, 0)
    } else {
        let idlq = if count(0) == 0 && count(1) > 0 {
            1
        } else if count(0) == 0 && count(1) == 0 {
            3
        } else {
            0
        };
        let run = rest
            .iter()
            .take(max - 1)
            .take_while(|id| id.idlq == idlq && id.low_preference == key.low_preference)
            .count();
        (idlq, run)
    };
    let mut fields = vec![(
        false,
        field(entry, idlq, key.low_preference, &ids[..1 + run]),
    )];
    let mut remaining = &rest[run..];
    while let Some(first) = remaining.first() {
        let run = remaining
            .iter()
            .take(max)
            .take_while(|id| id.idlq == first.idlq && id.low_preference == first.low_preference)
            .count();
        fields.push((
            true,
            field(entry, first.idlq, first.low_preference, &remaining[..run]),
        ));
        remaining = &remaining[run..];
    }
    fields
}

fn field(
    entry: &LinkageEntry,
    idlq: u8,
    low_preference: bool,
    ids: &[crate::config::linking::LinkId],
) -> Vec<u8> {
    let mut bytes = linking_flags(entry, true).to_vec();
    bytes.push((idlq << 5) | (u8::from(low_preference) << 4) | ids.len() as u8);
    for id in ids {
        if entry.data {
            bytes.extend_from_slice(&id.id.to_be_bytes());
        } else {
            if entry.international {
                bytes.push(id.ecc);
            }
            bytes.extend_from_slice(&(id.id as u16).to_be_bytes());
        }
    }
    bytes
}

/// FIG 0/21 fields: an FI list per chunk of frequencies.
pub fn frequency_fields(entry: &FiEntry) -> Vec<(bool, Vec<u8>)> {
    entry
        .frequencies
        .chunks(entry.per_list())
        .enumerate()
        .map(|(index, chunk)| {
            let frequencies: Vec<u8> = entry.id2.into_iter().chain(chunk.concat()).collect();
            (index > 0, fi_list(entry, entry.continuity, &frequencies))
        })
        .collect()
}

fn fi_list(entry: &FiEntry, continuity: bool, frequencies: &[u8]) -> Vec<u8> {
    let list_length = 3 + frequencies.len();
    let mut bytes = vec![0, list_length as u8];
    bytes.extend_from_slice(&entry.id.to_be_bytes());
    bytes.push(
        (entry.range_modulation << 4) | (u8::from(continuity) << 3) | frequencies.len() as u8,
    );
    bytes.extend_from_slice(frequencies);
    bytes
}

/// FIG 0/24 fields, at most as many EIds as fit a FIB.
pub fn oe_fields(entry: &OeEntry) -> Vec<(bool, Vec<u8>)> {
    let per_field = if entry.data { 11 } else { 12 };
    entry
        .eids
        .chunks(per_field)
        .enumerate()
        .map(|(index, eids)| {
            let mut bytes = sid_bytes(entry.data, entry.sid);
            bytes.push(eids.len() as u8);
            for eid in eids {
                bytes.extend_from_slice(&eid.to_be_bytes());
            }
            (index > 0, bytes)
        })
        .collect()
}

pub fn sci_field(entry: &SciEntry) -> Vec<u8> {
    let change = match entry.change {
        ChangeKind::Identity => 0,
        ChangeKind::Addition => 1,
        ChangeKind::LocalRemoval => 2,
        ChangeKind::GlobalRemoval => 3,
    };
    let mut bytes = sid_bytes(entry.data, entry.sid);
    bytes.push(
        (entry.scids << 4)
            | (change << 2)
            | (u8::from(entry.part_time) << 1)
            | u8::from(entry.description.is_some()),
    );
    if let Some((ca, data, scty)) = entry.description {
        bytes.push((u8::from(ca) << 7) | (u8::from(data) << 6) | scty);
    }
    let (date, hour, minute, second) = entry.date_time;
    bytes.extend_from_slice(&[
        (date << 3) | (hour >> 2),
        ((hour & 3) << 6) | minute,
        (second << 2)
            | (u8::from(entry.transfer_sid.is_some()) << 1)
            | u8::from(entry.transfer_eid.is_some()),
    ]);
    if let Some(sid) = entry.transfer_sid {
        bytes.extend(sid_bytes(entry.data, sid));
    }
    if let Some(eid) = entry.transfer_eid {
        bytes.extend_from_slice(&eid.to_be_bytes());
    }
    bytes
}

/// Database keys whose definitions are withheld while their CEI runs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Withheld {
    linkage: HashSet<(bool, bool, bool, u16)>,
    frequencies: HashSet<(bool, u16, u8)>,
    oe_services: HashSet<(bool, bool, u32)>,
}

/// Long-form definitions for the slow rotation.
pub fn definition_groups(databases: &Databases, withheld: &Withheld) -> Vec<Group> {
    let mut groups = grouped(
        databases
            .linkage
            .iter()
            .filter(|e| !withheld.linkage.contains(&e.key()))
            .flat_map(|entry| {
                linkage_fields(entry)
                    .into_iter()
                    .map(|(cn, bytes)| (header(cn, false, entry.data, EXT_LINKING), bytes))
            }),
    );
    groups.extend(grouped(
        databases
            .frequencies
            .iter()
            .filter(|e| !withheld.frequencies.contains(&e.key()))
            .flat_map(|entry| {
                frequency_fields(entry).into_iter().map(|(cn, bytes)| {
                    (
                        header(cn, entry.other_ensemble, false, EXT_FREQUENCIES),
                        bytes,
                    )
                })
            }),
    ));
    groups.extend(grouped(
        databases
            .other_ensembles
            .iter()
            .filter(|e| {
                !withheld
                    .oe_services
                    .contains(&(e.other_ensemble, e.data, e.sid))
            })
            .flat_map(|entry| {
                oe_fields(entry).into_iter().map(|(cn, bytes)| {
                    (
                        header(cn, entry.other_ensemble, entry.data, EXT_OE_SERVICES),
                        bytes,
                    )
                })
            }),
    ));
    groups
}

/// FIG 0/20, sent at once per second.
pub fn sci_groups(databases: &Databases) -> Vec<Group> {
    grouped(
        databases
            .changes
            .iter()
            .map(|entry| (header(false, false, entry.data, EXT_SCI), sci_field(entry))),
    )
}

/// Short-form signalling after a configuration change.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DatabaseEvents {
    groups: Vec<Group>,
    withheld: Withheld,
}

impl DatabaseEvents {
    /// CEIs for database entries that change or disappear, and activation
    /// state for linkage sets whose LA flag alone changes; deactivations are
    /// listed before activations (TS 103 176 clause 5.2.4.3).
    pub fn between(previous: &ValidatedConfig, next: &ValidatedConfig) -> Self {
        let (old, new) = (&previous.databases, &next.databases);
        let mut events = DatabaseEvents::default();
        let mut cei = Vec::new();
        let mut activation = Vec::new();
        for entry in &old.linkage {
            match new.linkage.iter().find(|e| e.key() == entry.key()) {
                Some(next) if next.ids == entry.ids && next.active != entry.active => {
                    activation.push((
                        next.active,
                        (
                            header(true, false, entry.data, EXT_LINKING),
                            linking_flags(next, false).to_vec(),
                        ),
                    ));
                }
                Some(next) if next.ids == entry.ids => {}
                next => {
                    if next.is_some() {
                        events.withheld.linkage.insert(entry.key());
                    }
                    cei.push((
                        header(false, false, entry.data, EXT_LINKING),
                        linking_flags(entry, false).to_vec(),
                    ));
                }
            }
        }
        activation.sort_by_key(|(active, _)| *active);
        for entry in &old.frequencies {
            if new.frequencies.iter().find(|e| e.key() == entry.key()) != Some(entry) {
                if new.frequencies.iter().any(|e| e.key() == entry.key()) {
                    events.withheld.frequencies.insert(entry.key());
                }
                cei.push((
                    header(false, entry.other_ensemble, false, EXT_FREQUENCIES),
                    fi_list(entry, false, &[]),
                ));
            }
        }
        for entry in &old.other_ensembles {
            let key = (entry.other_ensemble, entry.data, entry.sid);
            let next = new
                .other_ensembles
                .iter()
                .find(|e| (e.other_ensemble, e.data, e.sid) == key);
            if next != Some(entry) {
                if next.is_some() {
                    events.withheld.oe_services.insert(key);
                }
                let mut bytes = sid_bytes(entry.data, entry.sid);
                bytes.push(0);
                cei.push((
                    header(false, entry.other_ensemble, entry.data, EXT_OE_SERVICES),
                    bytes,
                ));
            }
        }
        events.groups = grouped(
            cei.into_iter()
                .chain(activation.into_iter().map(|(_, entry)| entry)),
        );
        events
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub(super) fn groups(&self) -> &[Group] {
        &self.groups
    }

    pub(super) fn withheld(&self) -> &Withheld {
        &self.withheld
    }
}
