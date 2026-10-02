//! FIC carousel (EN 300 401 clauses 5.2, 6 and 8).
//!
//! Each frame is filled in priority order:
//!
//! 1. FIG 0/0, then FIG 0/7, open the first FIB of each transmission frame
//!    (clause 6.4).
//! 2. FIG 0/10 every 96 ms, and one label (FIG 1/0, 1/1 or 1/5) per frame,
//!    placed in the last FIB with room.
//! 3. One complete pass of the MCI (FIG 0/1, 0/2, 0/3 and 0/14) per 96 ms,
//!    as clause 6.1 requires.
//! 4. The service following databases (FIG 0/6, 0/21, 0/24) get a steady
//!    share of the remaining space: each entry within 10 s, as TS 103 176
//!    asks, without starving the faster information.
//! 5. All remaining space carries FIG 0/5, 0/8, 0/9, 0/13, 0/17 and 0/20, and
//!    change indications for five seconds after a reload, in rotation; each
//!    must repeat at least once per second.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};

use crate::config::{
    encode_label, short_label_mask, SubchannelKind, UserApplication, ValidatedConfig,
};
use dabmux::frame::{cifs_per_transmission_frame, FrameClock};

/// FIG 0/13 user application types (TS 101 756 table 16).
const USER_APPLICATION_SLIDESHOW: u16 = 0x002;
const USER_APPLICATION_SPI: u16 = 0x007;
/// X-PAD application type "MOT, start of X-PAD data group" (EN 300 401 table 11).
const XPAD_APPTY_MOT_START: u8 = 12;
/// Data service component type MOT (TS 101 756 table 2b).
const DSCTY_MOT: u8 = 60;
/// SPI user application data: basic profile (TS 102 371 table 12).
const SPI_PROFILE_BASIC: u8 = 0x01;
/// FIG 0/14 FEC scheme of EN 300 401 clause 5.3.5.
const FEC_SCHEME_RS: u8 = 1;
/// Frames per MCI repetition period (96 ms).
const MCI_PERIOD_FRAMES: u64 = 4;
const FIB_DATA_BYTES: usize = 30;
/// Bytes per frame credited to the database rotation, about 170 bytes/s.
/// Credit accumulates up to a whole FIB so that large fields fit.
const DATABASE_BYTES_PER_FRAME: usize = 4;
/// Frames for one cycle of all labels, just under the required second.
const LABEL_CYCLE_FRAMES: u64 = 40;

mod database;
pub use database::DatabaseEvents;

pub struct FicCarousel {
    mci: Cursor,
    /// The 96 ms period whose MCI pass has been sent.
    mci_sent_for: Option<u64>,
    time_due: bool,
    label_cursor: usize,
    /// Frame count from which the next label may be sent.
    label_due: u64,
    information: Cursor,
    database: Cursor,
    database_credit: usize,
    events: DatabaseEvents,
    /// Frame count until which `events` are signalled.
    events_until: u64,
    reconfiguration_counter: Option<u16>,
}

impl Default for FicCarousel {
    fn default() -> Self {
        Self::new()
    }
}

impl FicCarousel {
    pub fn new() -> Self {
        Self {
            mci: Cursor::default(),
            mci_sent_for: None,
            time_due: false,
            label_cursor: 0,
            label_due: 0,
            information: Cursor::default(),
            database: Cursor::default(),
            database_credit: 0,
            events: DatabaseEvents::default(),
            events_until: 0,
            reconfiguration_counter: None,
        }
    }

    /// Signal `events` for five seconds from frame `count`, withholding the
    /// redefinitions they announce until then.
    pub fn with_database_events(mut self, events: DatabaseEvents, count: u64) -> Self {
        self.events = events;
        self.events_until = count + database::EVENT_FRAMES;
        self
    }

    /// Signal this FIG 0/7 count instead of the configured one. The runtime
    /// increments it on structural changes.
    pub fn with_reconfiguration_counter(mut self, counter: Option<u16>) -> Self {
        self.reconfiguration_counter = counter;
        self
    }

    pub fn write(&mut self, config: &ValidatedConfig, clock: FrameClock) -> Result<Vec<u8>> {
        let mode = config.source.ensemble.mode;
        let fib_count = if mode == 3 { 4 } else { 3 };
        let mut fibs = vec![Fib::default(); fib_count];

        if clock
            .count
            .is_multiple_of(cifs_per_transmission_frame(mode))
        {
            fibs[0].push(&fig0_0(config.source.ensemble.id, clock.count))?;
            let counter = self
                .reconfiguration_counter
                .or(config.source.ensemble.reconfiguration_counter);
            if let Some(counter) = counter {
                fibs[0].push(&fig0_7(counter, config.source.services.len() as u8))?;
            }
        }

        // Small, timed FIGs first; MCI then has the rest of the 96 ms period.
        if clock.count.is_multiple_of(MCI_PERIOD_FRAMES) {
            self.time_due = true;
        }
        if self.time_due {
            if let Some(fib) = fibs.iter_mut().find(|fib| fib.room() >= 8) {
                fib.push(&fig0_10(clock))?;
                self.time_due = false;
            }
        }
        // Labels are paced so that the full set takes about a second: more
        // often only wastes FIC capacity.
        let labels = 1 + config.source.services.len() + config.databases.labels.len();
        let pace = (LABEL_CYCLE_FRAMES / labels as u64).max(1);
        let label = self.label(config);
        if clock.count >= self.label_due {
            if let Some(fib) = fibs.iter_mut().rev().find(|fib| fib.room() >= label.len()) {
                fib.push(&label)?;
                self.label_cursor = (self.label_cursor + 1) % labels;
                self.label_due = clock.count + pace;
            }
        }

        let period = clock.count / MCI_PERIOD_FRAMES;
        if self.mci_sent_for != Some(period) {
            let groups = mci_groups(config);
            for fib in &mut fibs {
                if fill(fib, &groups, &mut self.mci, Fill::OnePass)? {
                    self.mci_sent_for = Some(period);
                    break;
                }
            }
        }

        let events_active = clock.count < self.events_until && !self.events.is_empty();
        let no_events = database::Withheld::default();
        let withheld = if events_active {
            self.events.withheld()
        } else {
            &no_events
        };
        let definitions = database::definition_groups(&config.databases, withheld);
        if !definitions.is_empty() {
            self.database_credit =
                (self.database_credit + DATABASE_BYTES_PER_FRAME).min(FIB_DATA_BYTES);
            for fib in fibs.iter_mut().rev() {
                let before = fib.used;
                fib.cap = (fib.used + self.database_credit).min(FIB_DATA_BYTES);
                fill(fib, &definitions, &mut self.database, Fill::Repeat)?;
                fib.cap = FIB_DATA_BYTES;
                self.database_credit -= fib.used - before;
            }
        }

        let mut groups = information_groups(config, clock);
        groups.extend(database::sci_groups(&config.databases));
        if events_active {
            groups.extend(self.events.groups().iter().cloned());
        }
        for fib in &mut fibs {
            fill(fib, &groups, &mut self.information, Fill::Repeat)?;
        }

        let mut output = Vec::with_capacity(fib_count * 32);
        for fib in fibs {
            output.extend_from_slice(&fib.finish());
        }
        Ok(output)
    }

    fn label(&self, config: &ValidatedConfig) -> Vec<u8> {
        let ensemble = &config.source.ensemble;
        if self.label_cursor == 0 {
            return fig1_label(
                0,
                &ensemble.id.to_be_bytes(),
                &ensemble.label,
                ensemble.short_label.as_deref(),
            );
        }
        let index = self.label_cursor - 1;
        let carried = config.source.services.len();
        if index >= carried {
            let announced = &config.databases.labels[index - carried];
            let (extension, id) = if announced.data {
                (5, announced.sid.to_be_bytes().to_vec())
            } else {
                (1, (announced.sid as u16).to_be_bytes().to_vec())
            };
            return fig1_label(
                extension,
                &id,
                &announced.label,
                announced.short_label.as_deref(),
            );
        }
        let service = &config.source.services[index];
        if config.services[index].data {
            fig1_label(
                5,
                &service.id.to_be_bytes(),
                &service.label,
                service.short_label.as_deref(),
            )
        } else {
            fig1_label(
                1,
                &(service.id as u16).to_be_bytes(),
                &service.label,
                service.short_label.as_deref(),
            )
        }
    }
}

#[derive(Clone)]
struct Fib {
    data: [u8; FIB_DATA_BYTES],
    used: usize,
    /// Bytes that may be used; lowered temporarily to budget a rotation.
    cap: usize,
}

impl Default for Fib {
    fn default() -> Self {
        Self {
            data: [0; FIB_DATA_BYTES],
            used: 0,
            cap: FIB_DATA_BYTES,
        }
    }
}

impl Fib {
    fn room(&self) -> usize {
        self.cap - self.used
    }

    fn push(&mut self, fig: &[u8]) -> Result<()> {
        if fig.len() > self.room() {
            bail!("FIB payload overflow");
        }
        self.data[self.used..self.used + fig.len()].copy_from_slice(fig);
        self.used += fig.len();
        Ok(())
    }

    /// The FIB with end marker, padding and CRC.
    fn finish(mut self) -> [u8; 32] {
        if self.used < FIB_DATA_BYTES {
            self.data[self.used] = 0xff;
        }
        let mut fib = [0u8; 32];
        fib[..FIB_DATA_BYTES].copy_from_slice(&self.data);
        fib[FIB_DATA_BYTES..].copy_from_slice(&dabmux::edi::crc16(&self.data).to_be_bytes());
        fib
    }
}

/// The entries of one type 0 FIG extension. Entries are packed into as few
/// FIGs as fit, but an entry is never split.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    /// Second header byte: C/N, OE, P/D and extension.
    extension: u8,
    entries: Vec<Vec<u8>>,
}

impl Group {
    fn new(extension: u8, data_services: bool, entries: Vec<Vec<u8>>) -> Self {
        Self {
            extension: extension | if data_services { 0x20 } else { 0 },
            entries,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Cursor {
    group: usize,
    entry: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// Stop when the last entry has been sent; returns true then.
    OnePass,
    /// Keep going round, but at most once per call.
    Repeat,
}

/// Pack entries from `cursor` into `fib` until it is full.
fn fill(fib: &mut Fib, groups: &[Group], cursor: &mut Cursor, mode: Fill) -> Result<bool> {
    if groups.iter().all(|group| group.entries.is_empty()) {
        *cursor = Cursor::default();
        return Ok(true);
    }
    let start = *cursor;
    let mut wrapped = false;
    loop {
        while groups
            .get(cursor.group)
            .is_some_and(|group| cursor.entry >= group.entries.len())
        {
            cursor.group += 1;
            cursor.entry = 0;
        }
        if cursor.group >= groups.len() {
            *cursor = Cursor::default();
            if mode == Fill::OnePass {
                return Ok(true);
            }
            wrapped = true;
            continue;
        }
        if wrapped && *cursor >= start {
            return Ok(false);
        }
        let group = &groups[cursor.group];
        let first = &group.entries[cursor.entry];
        if first.len() + 2 > FIB_DATA_BYTES {
            bail!("FIG 0/{} entry does not fit a FIB", group.extension & 0x1f);
        }
        if fib.room() < first.len() + 2 {
            return Ok(false);
        }
        let mut fig = vec![0, group.extension];
        while let Some(entry) = group.entries.get(cursor.entry) {
            if fig.len() + entry.len() > fib.room() {
                break;
            }
            fig.extend_from_slice(entry);
            cursor.entry += 1;
        }
        fig[0] = (fig.len() - 1) as u8;
        fib.push(&fig)?;
    }
}

/// FIG 0/1, 0/2 (programme, then data services), 0/3 and 0/14.
fn mci_groups(config: &ValidatedConfig) -> Vec<Group> {
    let subchannels = config
        .subchannels
        .iter()
        .map(|sub| {
            let sad = sub.start_address_cu;
            let size = sub.size_cu;
            let option = (sub.tpl >> 2) & 1;
            let level = sub.tpl & 3;
            vec![
                (sub.id << 2) | ((sad >> 8) as u8 & 3),
                sad as u8,
                0x80 | (option << 4) | (level << 2) | ((size >> 8) as u8 & 3),
                size as u8,
            ]
        })
        .collect();
    let mut programme = Vec::new();
    let mut data = Vec::new();
    for (index, service) in config.source.services.iter().enumerate() {
        let is_data = config.services[index].data;
        let mut entry = if is_data {
            service.id.to_be_bytes().to_vec()
        } else {
            (service.id as u16).to_be_bytes().to_vec()
        };
        let components: Vec<_> = config.service_components(index).collect();
        entry.push(components.len() as u8);
        for component in components {
            let primary = u8::from(component.scids == 0) << 1;
            match (
                config.source.subchannels[component.subchannel].kind,
                component.scid,
            ) {
                (SubchannelKind::EnhancedPacket, Some(scid)) => {
                    entry.extend_from_slice(&[
                        0xc0 | (scid >> 6) as u8,
                        ((scid & 0x3f) as u8) << 2 | primary,
                    ]);
                }
                (kind, _) => {
                    let ascty = if kind == SubchannelKind::DabPlus {
                        0x3f
                    } else {
                        0
                    };
                    let subchid = config.subchannels[component.subchannel].id;
                    entry.extend_from_slice(&[ascty, (subchid << 2) | primary]);
                }
            }
        }
        if is_data {
            data.push(entry);
        } else {
            programme.push(entry);
        }
    }
    let packet_components = config
        .components
        .iter()
        .zip(&config.source.components)
        .filter_map(|(component, raw)| {
            let packet = raw.packet.as_ref()?;
            let scid = component.scid?;
            let subchid = config.subchannels[component.subchannel].id;
            Some(vec![
                (scid >> 4) as u8,
                ((scid & 0x0f) as u8) << 4,
                // DG flag 0 means data groups are used.
                (u8::from(!packet.data_groups) << 7) | packet.dscty,
                (subchid << 2) | (packet.address >> 8) as u8,
                packet.address as u8,
            ])
        })
        .collect();
    let fec = config
        .subchannels
        .iter()
        .zip(&config.source.subchannels)
        .filter(|(_, raw)| raw.kind == SubchannelKind::EnhancedPacket)
        .map(|(sub, _)| vec![(sub.id << 2) | FEC_SCHEME_RS])
        .collect();
    vec![
        Group::new(1, false, subchannels),
        Group::new(2, false, programme),
        Group::new(2, true, data),
        Group::new(3, false, packet_components),
        Group::new(14, false, fec),
    ]
}

/// FIG 0/5, 0/17, 0/9, 0/13 and 0/8, the latter two split by P/D.
fn information_groups(config: &ValidatedConfig, clock: FrameClock) -> Vec<Group> {
    let source = &config.source;
    let sid = |index: usize| -> Vec<u8> {
        if config.services[index].data {
            source.services[index].id.to_be_bytes().to_vec()
        } else {
            (source.services[index].id as u16).to_be_bytes().to_vec()
        }
    };
    // One FIG 0/5 entry per audio subchannel; validation rejects conflicts.
    let mut signalled = HashSet::new();
    let languages = config
        .components
        .iter()
        .filter_map(|component| {
            let language = source.services[component.service].language;
            let sub = &config.subchannels[component.subchannel];
            let audio = source.subchannels[component.subchannel].kind.is_audio();
            (language != 0 && audio && signalled.insert(component.subchannel))
                .then(|| vec![sub.id & 0x3f, language])
        })
        .collect();
    let programme_types = source
        .services
        .iter()
        .filter(|service| service.pty != 0)
        .map(|service| {
            let [high, low] = (service.id as u16).to_be_bytes();
            vec![high, low, 0, service.pty]
        })
        .collect();
    let mut applications = [Vec::new(), Vec::new()];
    let mut definitions = [Vec::new(), Vec::new()];
    for (component, raw) in config.components.iter().zip(&source.components) {
        let data = config.services[component.service].data;
        let audio = source.subchannels[component.subchannel].kind.is_audio();
        if !raw.user_applications.is_empty() {
            let mut entry = sid(component.service);
            entry.push(component.scids << 4 | raw.user_applications.len() as u8);
            for application in &raw.user_applications {
                // X-PAD data for applications in an audio component's PAD.
                let (kind, mut payload) = match application {
                    UserApplication::Slideshow if audio => (
                        USER_APPLICATION_SLIDESHOW,
                        vec![XPAD_APPTY_MOT_START, DSCTY_MOT],
                    ),
                    UserApplication::Slideshow => (USER_APPLICATION_SLIDESHOW, Vec::new()),
                    UserApplication::Spi => (USER_APPLICATION_SPI, vec![SPI_PROFILE_BASIC]),
                };
                let [high, low] = (kind << 5 | payload.len() as u16).to_be_bytes();
                entry.extend_from_slice(&[high, low]);
                entry.append(&mut payload);
            }
            applications[usize::from(data)].push(entry);
        }
        // Required for secondary components and for components with user
        // applications (clause 6.3.5).
        if component.scids > 0 || !raw.user_applications.is_empty() {
            let mut entry = sid(component.service);
            entry.push(component.scids & 0x0f);
            match component.scid {
                Some(scid) => entry.extend_from_slice(&[0x80 | (scid >> 8) as u8, scid as u8]),
                None => entry.push(config.subchannels[component.subchannel].id & 0x3f),
            }
            definitions[usize::from(data)].push(entry);
        }
    }
    let [programme_applications, data_applications] = applications;
    let [programme_definitions, data_definitions] = definitions;
    vec![
        Group::new(5, false, languages),
        Group::new(17, false, programme_types),
        Group::new(9, false, vec![fig0_9_entry(config, clock)]),
        Group::new(13, false, programme_applications),
        Group::new(13, true, data_applications),
        Group::new(8, false, programme_definitions),
        Group::new(8, true, data_definitions),
    ]
}

fn fig0_0(eid: u16, count: u64) -> [u8; 6] {
    let high = ((count / 250) % 20) as u8;
    let low = (count % 250) as u8;
    let [e0, e1] = eid.to_be_bytes();
    [5, 0, e0, e1, high, low]
}

/// Country, LTO and international table, with the extended field listing
/// services whose ECC differs from the ensemble's.
fn fig0_9_entry(config: &ValidatedConfig, clock: FrameClock) -> Vec<u8> {
    static LOOKUP_FAILED: AtomicBool = AtomicBool::new(false);
    let ensemble = &config.source.ensemble;
    let lto = if ensemble.local_time_offset_auto {
        match crate::timing::local_offset_half_hours(clock.unix_seconds) {
            Ok(lto) => {
                LOOKUP_FAILED.store(false, Ordering::Relaxed);
                lto
            }
            Err(err) => {
                if !LOOKUP_FAILED.swap(true, Ordering::Relaxed) {
                    tracing::warn!(%err, "local time offset lookup failed; signalling UTC");
                }
                0
            }
        }
    } else {
        ensemble.local_time_offset_half_hours
    };
    let lto_field = if lto < 0 {
        ((-lto) as u8) | 0x20
    } else {
        lto as u8
    };
    let mut by_ecc: Vec<(u8, Vec<u16>)> = Vec::new();
    for (service, validated) in config.source.services.iter().zip(&config.services) {
        if let Some(ecc) = validated.foreign_ecc {
            match by_ecc.iter_mut().find(|(key, _)| *key == ecc) {
                Some((_, ids)) => ids.push(service.id as u16),
                None => by_ecc.push((ecc, vec![service.id as u16])),
            }
        }
    }
    let mut extended = Vec::new();
    for (ecc, ids) in &by_ecc {
        for chunk in ids.chunks(3) {
            extended.push((chunk.len() as u8) << 6);
            extended.push(*ecc);
            for id in chunk {
                extended.extend_from_slice(&id.to_be_bytes());
            }
        }
    }
    let ext_flag = if extended.is_empty() { 0 } else { 0x80 };
    let mut entry = vec![
        ext_flag | lto_field,
        ensemble.ecc,
        ensemble.international_table,
    ];
    entry.extend(extended);
    entry
}

fn fig0_7(counter: u16, service_count: u8) -> [u8; 4] {
    [
        3,
        7,
        (service_count << 2) | ((counter >> 8) as u8 & 3),
        counter as u8,
    ]
}

fn fig0_10(clock: FrameClock) -> [u8; 8] {
    let mjd = (clock.unix_seconds.div_euclid(86_400) + 40_587) as u32;
    let sod = clock.unix_seconds.rem_euclid(86_400) as u32;
    let hour = sod / 3600;
    let minute = (sod / 60) % 60;
    let second = sod % 60;
    let millis = u32::from(clock.millisecond);
    [
        7,
        10,
        ((mjd >> 10) & 0x7f) as u8,
        ((mjd >> 2) & 0xff) as u8,
        (((mjd & 3) << 6) | ((hour >> 2) & 7) | 0x18) as u8,
        ((minute & 0x3f) | ((hour & 3) << 6)) as u8,
        (((second & 0x3f) << 2) | ((millis >> 8) & 3)) as u8,
        millis as u8,
    ]
}

/// FIG 1 label: ensemble (1/0), programme (1/1, 16-bit SId) or data service
/// (1/5, 32-bit SId).
fn fig1_label(extension: u8, id: &[u8], label: &str, short_label: Option<&str>) -> Vec<u8> {
    let mut fig = vec![0u8; 2 + id.len() + 18];
    fig[0] = (1 << 5) | (fig.len() - 1) as u8;
    fig[1] = extension;
    fig[2..2 + id.len()].copy_from_slice(id);
    let text = &mut fig[2 + id.len()..2 + id.len() + 16];
    text.fill(b' ');
    let encoded = encode_label("label", label).expect("validated label");
    text[..encoded.len()].copy_from_slice(&encoded);
    let mask = short_label_mask("short_label", label, short_label).expect("validated short label");
    let end = fig.len();
    fig[end - 2..].copy_from_slice(&mask.to_be_bytes());
    fig
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::{ComponentConfig, InputSpec, SubchannelConfig};
    use crate::config::testing::{dab_plus, example, reference, service};
    use crate::config::{parse_yaml, Config};
    use std::collections::HashMap;

    /// A FIG as read back from a FIB: type, extension, P/D flag and body.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Fig {
        kind: u8,
        extension: u8,
        pd: bool,
        /// Whole FIG, header included.
        bytes: Vec<u8>,
    }

    impl Fig {
        fn body(&self) -> &[u8] {
            if self.kind == 0 {
                &self.bytes[2..]
            } else {
                &self.bytes[1..]
            }
        }
    }

    /// Split each FIB into FIGs, checking CRCs and end markers.
    fn figs(fic: &[u8]) -> Vec<Vec<Fig>> {
        fic.chunks(32)
            .map(|fib| {
                assert_eq!(&fib[30..], &dabmux::edi::crc16(&fib[..30]).to_be_bytes());
                let mut figs = Vec::new();
                let mut offset = 0;
                while offset < 30 && fib[offset] != 0xff {
                    let length = usize::from(fib[offset] & 0x1f);
                    let bytes = fib[offset..offset + 1 + length].to_vec();
                    let kind = bytes[0] >> 5;
                    let (extension, pd) = if kind == 0 {
                        (bytes[1] & 0x1f, bytes[1] & 0x20 != 0)
                    } else {
                        (bytes[1] & 0x07, false)
                    };
                    figs.push(Fig {
                        kind,
                        extension,
                        pd,
                        bytes,
                    });
                    offset += 1 + length;
                }
                figs
            })
            .collect()
    }

    /// Split a FIG body into entry keys, read per EN 300 401 independently
    /// of the writer, so repetition can be measured per entry.
    fn entries(fig: &Fig) -> Vec<String> {
        let body = fig.body();
        let sid_len = if fig.pd { 4 } else { 2 };
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let mut keys = Vec::new();
        let mut reader = Reader { body, offset: 0 };
        match (fig.kind, fig.extension) {
            (0, 1) => {
                while reader.more() {
                    let long = reader.peek(2) & 0x80 != 0;
                    let entry = reader.take(if long { 4 } else { 3 });
                    keys.push(format!("0/1 {}", entry[0] >> 2));
                }
            }
            (0, 2) => {
                while reader.more() {
                    let sid = hex(reader.take(sid_len));
                    let count = usize::from(reader.take(1)[0] & 0x0f);
                    reader.take(2 * count);
                    keys.push(format!("0/2 {sid}"));
                }
            }
            (0, 3) => {
                while reader.more() {
                    let entry = reader.take(5);
                    keys.push(format!(
                        "0/3 {}",
                        (u16::from(entry[0]) << 4) | u16::from(entry[1] >> 4)
                    ));
                }
            }
            (0, 14) => {
                while reader.more() {
                    keys.push(format!("0/14 {}", reader.take(1)[0] >> 2));
                }
            }
            (0, 5) => {
                while reader.more() {
                    keys.push(format!("0/5 {}", reader.take(2)[0] & 0x3f));
                }
            }
            (0, 17) => {
                while reader.more() {
                    keys.push(format!("0/17 {}", hex(&reader.take(4)[..2])));
                }
            }
            (0, 8) => {
                while reader.more() {
                    let sid = hex(reader.take(sid_len));
                    let scids = reader.take(1)[0] & 0x0f;
                    let long = reader.peek(0) & 0x80 != 0;
                    reader.take(if long { 2 } else { 1 });
                    keys.push(format!("0/8 {sid}/{scids}"));
                }
            }
            (0, 13) => {
                while reader.more() {
                    let sid = hex(reader.take(sid_len));
                    let header = reader.take(1)[0];
                    for _ in 0..header & 0x0f {
                        let length = usize::from(reader.take(2)[1] & 0x1f);
                        reader.take(length);
                    }
                    keys.push(format!("0/13 {sid}/{}", header >> 4));
                }
            }
            (0, 6) | (0, 20) | (0, 21) | (0, 24) => {
                for field in fields(fig) {
                    let key = match fig.extension {
                        // C/N and the database key, not the content.
                        6 => format!(
                            "{:02x} {}",
                            fig.bytes[1] & 0xa0,
                            hex(&[field[0] & 0x1f, field[1]])
                        ),
                        20 => hex(&field[..sid_len + 1]),
                        21 => format!(
                            "{:02x} {}",
                            fig.bytes[1] & 0x40,
                            hex(&[field[2], field[3], field[4] >> 4])
                        ),
                        _ => format!("{:02x} {}", fig.bytes[1] & 0x60, hex(&field[..sid_len])),
                    };
                    keys.push(format!("0/{} {key}", fig.extension));
                }
            }
            (0, 9) => keys.push("0/9".into()),
            (0, 10) => keys.push("0/10".into()),
            (0, 0) => keys.push("0/0".into()),
            (0, 7) => keys.push("0/7".into()),
            (1, extension) => {
                let id_len = if extension == 5 { 4 } else { 2 };
                keys.push(format!("1/{extension} {}", hex(&body[1..1 + id_len])));
            }
            other => panic!("unexpected FIG {other:?}"),
        }
        keys
    }

    /// Fields of FIG 0/6, 0/20, 0/21 and 0/24, per EN 300 401 figures 53,
    /// 40, 46 and 47.
    fn fields(fig: &Fig) -> Vec<Vec<u8>> {
        let body = fig.body();
        let sid_len = if fig.pd { 4 } else { 2 };
        let mut reader = Reader { body, offset: 0 };
        let mut fields = Vec::new();
        while reader.more() {
            let start = reader.offset;
            match fig.extension {
                6 => {
                    let flags = reader.take(2);
                    if flags[0] & 0x80 != 0 {
                        let count = usize::from(reader.take(1)[0] & 0x0f);
                        let size = if fig.pd {
                            4
                        } else if flags[0] & 0x10 != 0 {
                            3
                        } else {
                            2
                        };
                        reader.take(count * size);
                    }
                }
                20 => {
                    reader.take(sid_len);
                    let flags = reader.take(1)[0];
                    if flags & 1 != 0 {
                        reader.take(1);
                    }
                    let time = reader.take(3);
                    if time[2] & 2 != 0 {
                        reader.take(sid_len);
                    }
                    if time[2] & 1 != 0 {
                        reader.take(2);
                    }
                }
                21 => {
                    let length = usize::from(reader.take(2)[1] & 0x1f);
                    reader.take(length);
                }
                24 => {
                    reader.take(sid_len);
                    let count = usize::from(reader.take(1)[0] & 0x0f);
                    reader.take(2 * count);
                }
                other => panic!("no field layout for FIG 0/{other}"),
            }
            fields.push(body[start..reader.offset].to_vec());
        }
        fields
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    struct Reader<'a> {
        body: &'a [u8],
        offset: usize,
    }

    impl<'a> Reader<'a> {
        fn more(&self) -> bool {
            self.offset < self.body.len()
        }

        fn peek(&self, index: usize) -> u8 {
            self.body[self.offset + index]
        }

        fn take(&mut self, n: usize) -> &'a [u8] {
            let part = &self.body[self.offset..self.offset + n];
            self.offset += n;
            part
        }
    }

    /// All FIGs of `frames` consecutive frames starting at `count` 0.
    fn run(config: &ValidatedConfig, frames: u64) -> Vec<Vec<Fig>> {
        let mut carousel = FicCarousel::new();
        (0..frames)
            .map(|count| {
                let clock =
                    FrameClock::new(count, 1_704_164_645, (count % 41) as u16 * 24).unwrap();
                figs(&carousel.write(config, clock).unwrap()).concat()
            })
            .collect()
    }

    /// mux-zh: 17 DAB+ services with slideshow, BOLLERWAGEN's foreign ECC
    /// and the SPI data service.
    fn mux_zh() -> ValidatedConfig {
        parse_yaml(include_str!("../config.mux-zh.example.yaml"))
            .unwrap()
            .validate()
            .unwrap()
    }

    fn expected_mci(config: &ValidatedConfig) -> HashSet<String> {
        let mut keys = HashSet::new();
        for sub in &config.subchannels {
            keys.insert(format!("0/1 {}", sub.id));
        }
        for (service, validated) in config.source.services.iter().zip(&config.services) {
            let sid = if validated.data {
                format!("{:08x}", service.id)
            } else {
                format!("{:04x}", service.id)
            };
            keys.insert(format!("0/2 {sid}"));
        }
        for (component, raw) in config.components.iter().zip(&config.source.components) {
            if raw.packet.is_some() {
                keys.insert(format!("0/3 {}", component.scid.unwrap()));
                keys.insert(format!(
                    "0/14 {}",
                    config.subchannels[component.subchannel].id
                ));
            }
        }
        keys
    }

    #[test]
    fn mci_is_complete_in_every_96_ms_period() {
        for config in [example().validate().unwrap(), mux_zh()] {
            let expected = expected_mci(&config);
            for (period, frames) in run(&config, 400).chunks(4).enumerate() {
                let sent: HashSet<String> = frames.iter().flatten().flat_map(entries).collect();
                let missing: Vec<_> = expected.difference(&sent).collect();
                assert!(missing.is_empty(), "period {period} misses {missing:?}");
            }
        }
    }

    #[test]
    fn information_and_labels_repeat_at_least_once_per_second() {
        let config = mux_zh();
        let frames = run(&config, 1000);
        let mut seen: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, figs) in frames.iter().enumerate() {
            for key in figs.iter().flat_map(entries) {
                seen.entry(key).or_default().push(index);
            }
        }
        // Every service has a label, slideshow or SPI signalling and FIG 0/8;
        // programme services also language and PTY.
        let count = |prefix: &str| seen.keys().filter(|key| key.starts_with(prefix)).count();
        assert_eq!(
            [
                count("1/1 "),
                count("1/5 "),
                count("0/13 "),
                count("0/8 "),
                count("0/5 "),
                count("0/17 ")
            ],
            // srv-soutak has no language and srv-suissepodcast no PTY.
            [17, 1, 18, 18, 16, 16]
        );
        // 41 frames are 984 ms; the first sighting must come within a second too.
        for (key, frames) in &seen {
            let gaps = std::iter::once(frames[0] + 1).chain(frames.windows(2).map(|w| w[1] - w[0]));
            let worst = gaps.max().unwrap();
            assert!(worst <= 41, "{key} repeats only every {worst} frames");
        }
        assert!(
            seen.contains_key("0/9") && seen.contains_key("0/10") && seen.contains_key("1/0 4401")
        );
    }

    #[test]
    fn fig_bytes_match_odr_dabmux_for_the_spi_reference() {
        // devsupport/cpp-reference/spi.mux, captured from ODR-DabMux v5.5.1.
        let config = parse_yaml(
            r#"
ensemble: {id: 0x4401, ecc: 0xe1, label: DIG D04 - ZH, short_label: DIG D04,
           local_time_offset_half_hours: 4, international_table: 1}
services:
  - id: 0x1498
    ecc: 0xe0
    label: BOLLERWAGEN
    short_label: BOLLER
    pty: 10
    language: 0x08
    components:
      - {type: dab_plus, subchannel_id: 10, bitrate: 72, user_applications: [slideshow],
         input: {protocol: edi, uri: "tcp://:9055"}}
  - id: 0x44010001
    label: DIG_SPI
    short_label: DIG_SPI
    components:
      - {type: enhanced_packet, subchannel_id: 30, bitrate: 8, packet_address: 1,
         user_applications: [spi], input: {protocol: file, path: spi.bin}}
output: {destinations: [{protocol: tcp, listen_port: 8850}]}
"#,
        )
        .unwrap()
        .validate()
        .unwrap();
        let sent: HashSet<String> = run(&config, 100)
            .iter()
            .flatten()
            .map(|fig| fig.bytes.iter().map(|b| format!("{b:02x}")).collect())
            .collect();
        for reference in [
            "06021498013f2a",                                   // FIG 0/2, programme service
            "08224401000101c002", // FIG 0/2, data service, packet component SCId 0
            "060300003c7801",     // FIG 0/3, MOT, data groups, SubChId 30, address 1
            "020e79",             // FIG 0/14, SubChId 30, FEC scheme 1
            "03050a08",           // FIG 0/5
            "05081498000a",       // FIG 0/8, short form
            "082844010001008000", // FIG 0/8, data service, long form
            "080984e10140e01498", // FIG 0/9 with 0x1498 under ECC 0xE0
            "080d14980100420c3c", // FIG 0/13, slideshow in X-PAD
            "092d440100010100e101", // FIG 0/13, SPI basic profile
            "3500440144494720443034202d205a4820202020fe00", // FIG 1/0
            "35011498424f4c4c4552574147454e2020202020fc00", // FIG 1/1
            "3705440100014449475f535049202020202020202020fe00", // FIG 1/5
        ] {
            assert!(sent.contains(reference), "missing {reference}");
        }
    }

    #[test]
    fn ensemble_information_opens_each_transmission_frame_only() {
        let mut config = example();
        config.ensemble.reconfiguration_counter = Some(0x123);
        let valid = config.clone().validate().unwrap();
        let mut carousel = FicCarousel::new().with_reconfiguration_counter(Some(0x124));
        let first = carousel
            .write(&valid, FrameClock::new(8, 0, 0).unwrap())
            .unwrap();
        assert_eq!(&first[..10], &[5, 0, 0x4f, 0xff, 0, 8, 3, 7, 5, 0x24]);
        let second = carousel
            .write(&valid, FrameClock::new(9, 0, 0).unwrap())
            .unwrap();
        assert_ne!(&second[..2], &[5, 0], "FIG 0/0 only at frame phase 0");

        // Mode IV has 48 ms transmission frames; modes II and III 24 ms.
        for (mode, count, expected) in [(4, 2, true), (4, 3, false), (2, 1, true), (3, 1, true)] {
            config.ensemble.mode = mode;
            let valid = config.clone().validate().unwrap();
            let fic = FicCarousel::new()
                .write(&valid, FrameClock::new(count, 0, 0).unwrap())
                .unwrap();
            assert_eq!(fic[..2] == [5, 0], expected, "mode {mode} frame {count}");
        }
    }

    #[test]
    fn entries_pack_into_shared_figs_and_resume_in_the_next_fib() {
        let groups = [
            Group::new(
                2,
                false,
                vec![vec![0x4d, 0xa4, 1, 0x3f, 6], vec![0x4d, 0xa5, 1, 0x3f, 6]],
            ),
            Group::new(14, false, vec![vec![0x79]; 10]),
        ];
        let mut cursor = Cursor::default();
        let mut fib = Fib::default();
        fib.push(&[0; 14]).unwrap();
        assert!(!fill(&mut fib, &groups, &mut cursor, Fill::OnePass).unwrap());
        // 16 bytes left: one FIG 0/2 with both services, then FIG 0/14 with
        // the four entries that still fit.
        assert_eq!(
            &fib.data[14..26],
            &[11, 2, 0x4d, 0xa4, 1, 0x3f, 6, 0x4d, 0xa5, 1, 0x3f, 6]
        );
        assert_eq!(&fib.data[26..], &[3, 14, 0x79, 0x79]);
        let mut next = Fib::default();
        assert!(fill(&mut next, &groups, &mut cursor, Fill::OnePass).unwrap());
        assert_eq!(
            &next.data[..10],
            &[9, 14, 0x79, 0x79, 0x79, 0x79, 0x79, 0x79, 0x79, 0x79]
        );
        assert_eq!(cursor, Cursor::default());
    }

    #[test]
    fn packet_components_of_programme_services() {
        // A packet component in a programme service: TMId 3 in a P/D = 0
        // FIG 0/2, and FIG 0/8 in long form for the secondary component.
        let mut config = example();
        config.services[0].components.push(ComponentConfig {
            kind: Some(SubchannelKind::EnhancedPacket),
            bitrate: Some(8),
            subchannel_id: Some(5),
            input: Some(InputSpec::File {
                path: "data.bin".into(),
            }),
            packet_address: Some(7),
            dscty: Some(5),
            data_groups: Some(false),
            ..Default::default()
        });
        let valid = config.validate().unwrap();
        assert!(!valid.services[0].data);
        let sent: HashSet<Vec<u8>> = run(&valid, 50)
            .iter()
            .flatten()
            .map(|fig| fig.bytes.clone())
            .collect();
        assert!(sent.contains(&vec![8, 2, 0x4d, 0xa4, 2, 0x3f, 6, 0xc0, 0]));
        assert!(
            sent.contains(&vec![6, 3, 0, 0, 0x85, 0x14, 7]),
            "TDC without data groups"
        );
        assert!(sent.contains(&vec![6, 8, 0x4d, 0xa4, 1, 0x80, 0]));
    }

    #[test]
    fn fig_zero_nine_groups_foreign_eccs_three_per_sub_field() {
        let mut config = example();
        for index in 0..4u16 {
            let mut station = service(
                0x1101 + u32::from(index),
                &format!("Ausland {index}"),
                vec![dab_plus(32, &format!("udp://127.0.0.1:{}", 9100 + index))],
            );
            station.ecc = Some(0xe0);
            config.services.push(station);
        }
        config.services[0].ecc = Some(0xe1); // same as the ensemble: not listed
        let valid = config.validate().unwrap();
        let entry = fig0_9_entry(&valid, FrameClock::new(0, 0, 0).unwrap());
        assert_eq!(
            entry,
            [
                0x80, 0xe1, 0, 0xc0, 0xe0, 0x11, 0x01, 0x11, 0x02, 0x11, 0x03, 0x40, 0xe0, 0x11,
                0x04
            ]
        );
    }

    #[test]
    fn shared_subchannel_language_is_signalled_once() {
        let mut config = example();
        config.services[0].language = 8;
        share_with_second_service(&mut config, 8);
        let languages: Vec<Vec<u8>> = run(&config.validate().unwrap(), 41)
            .iter()
            .flatten()
            .filter(|fig| (fig.kind, fig.extension) == (0, 5))
            .map(|fig| fig.body().to_vec())
            .collect();
        assert!(!languages.is_empty());
        assert!(languages.iter().all(|body| body == &[1, 8]));
    }

    #[test]
    fn explicit_short_label_mask_is_emitted_in_fig_one() {
        let mut config = example();
        config.ensemble.label = "RND D00 - XX".into();
        config.ensemble.short_label = Some("RND D00".into());
        config.services[0].label = "105 DJ HRND-001".into();
        config.services[0].short_label = Some("HRND-001".into());
        let labels: HashMap<u8, Vec<u8>> = run(&config.validate().unwrap(), 41)
            .iter()
            .flatten()
            .filter(|fig| fig.kind == 1)
            .map(|fig| (fig.extension, fig.bytes[fig.bytes.len() - 2..].to_vec()))
            .collect();
        assert_eq!(labels[&0], 0xfe00u16.to_be_bytes());
        assert_eq!(labels[&1], 0x01feu16.to_be_bytes());
    }

    #[test]
    fn slideshow_on_a_secondary_component_gets_fig_zero_eight() {
        let mut config = example();
        let mut second = dab_plus(64, "udp://127.0.0.1:9002");
        second.subchannel_id = Some(2);
        second.user_applications = vec![crate::config::UserApplication::Slideshow];
        config.services[0].components.push(second);
        let sent: HashSet<Vec<u8>> = run(&config.validate().unwrap(), 41)
            .iter()
            .flatten()
            .map(|fig| fig.bytes.clone())
            .collect();
        assert!(
            sent.contains(&vec![5, 8, 0x4d, 0xa4, 1, 2]),
            "FIG 0/8 short form, SCIdS 1"
        );
        assert!(sent.contains(&vec![8, 13, 0x4d, 0xa4, 0x11, 0, 0x42, 12, 60]));
    }

    fn service_linking() -> ValidatedConfig {
        parse_yaml(include_str!("../config.service-linking.example.yaml"))
            .unwrap()
            .validate()
            .unwrap()
    }

    /// Every field sent in `frames` frames, as "<second header byte>:<field>".
    fn sent_fields(config: &ValidatedConfig, frames: u64) -> HashSet<String> {
        run(config, frames)
            .iter()
            .flatten()
            .filter(|fig| fig.kind == 0 && matches!(fig.extension, 6 | 20 | 21 | 24))
            .flat_map(|fig| {
                fields(fig)
                    .into_iter()
                    .map(move |field| format!("{:02x}:{}", fig.bytes[1], hex(&field)))
            })
            .collect()
    }

    #[test]
    fn service_following_matches_odr_dabmux_where_it_follows_the_standard() {
        // devsupport/cpp-reference/service-linking.mux, ODR-DabMux v5.5.1.
        let sent = sent_fields(&service_linking(), 200);
        for reference in [
            "06:eabc028daa8daf",         // FIG 0/6 set-fu start: key and DAB link
            "86:eabc211a2b",             // continuation: RDS PI code
            "15:00094fff0e10392e102e20", // FIG 0/21 own ensemble, two frequencies
            "95:00064fff0b183858",       // and the third in a continuation
            "55:00064fee0b183858",       // other ensemble
            "15:0008ab456d123b6a5780",   // DRM, two frequencies in kHz
            "15:0006cc88eb3339d0",       // AMSS
            "18:8daa034ffe4ffd4fff",     // FIG 0/24, OE = 0
            "14:123404fffffc",           // FIG 0/20 entries
            "14:56780cfffffc",
            "14:abcd00fffffeef01",
            "14:111100ffffff22223333",
            "14:123400fffffec012",
            "14:44440700fffffc",
        ] {
            assert!(sent.contains(reference), "missing {reference}");
        }
        // Where ODR-DabMux departs from the standards:
        // - set-ri: soft ("hard soft" parses as hard in C++), DAB/RDS before
        //   DRM/AMSS, and DRM and AMSS share IdLQ 11 (TS 103 176 5.2.4.1).
        assert!(sent.contains("06:ddef22ec8dab4f4c5d"), "start: key and RDS");
        assert!(
            sent.contains("86:ddef62ec1298ea1a2b"),
            "continuation: DRM and AMSS"
        );
        // - 87.6 and 105.2 MHz are FM codes 1 and 177; C++ truncates to 0 and
        //   176, and sends this new database key with C/N = 1.
        assert!(sent.contains("15:000512348a01b1"));
        // - FIG 0/24 for 0x8DAF starts its own database entry: C/N = 0.
        assert!(!sent.contains("d8:8daf014ffd") && sent.contains("58:8daf014ffd"));
    }

    #[test]
    fn databases_meet_their_rates_beside_a_busy_ensemble() {
        let mut config = parse_yaml(include_str!("../config.mux-zh.example.yaml")).unwrap();
        for (index, service) in config
            .services
            .iter_mut()
            .enumerate()
            .filter(|(_, s)| s.id <= 0xffff)
        {
            let index = index as u32;
            service.linking = vec![crate::config::linking::LinkageSet {
                lsn: 0x100 + index as u16,
                hard: true,
                active: true,
                international: None,
                links: (0..4)
                    .map(|n| crate::config::linking::Link {
                        kind: if n < 2 {
                            crate::config::linking::LinkKind::Dab
                        } else {
                            crate::config::linking::LinkKind::Fm
                        },
                        id: 0x5000 + index * 4 + n,
                        ecc: None,
                        preference: Default::default(),
                    })
                    .collect(),
            }];
            service.other_ensembles = vec![0x4402, 0x4403, 0x4404];
        }
        let config = config.validate().unwrap();
        let frames = run(&config, 2000);
        let mut seen: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, figs) in frames.iter().enumerate() {
            for key in figs.iter().flat_map(entries) {
                seen.entry(key).or_default().push(index);
            }
        }
        let worst = |key: &String| {
            let frames = &seen[key];
            std::iter::once(frames[0] + 1)
                .chain(frames.windows(2).map(|w| w[1] - w[0]))
                .max()
                .unwrap()
        };
        assert_eq!(
            seen.keys().filter(|k| k.starts_with("0/6 ")).count(),
            2 * 17,
            "start and continuation"
        );
        for key in seen.keys() {
            // Database entries within 10 s (417 frames), the rest within 1 s.
            let limit = if key.starts_with("0/6 ") || key.starts_with("0/24 ") {
                416
            } else {
                41
            };
            assert!(
                worst(key) <= limit,
                "{key} repeats only every {} frames",
                worst(key)
            );
        }
    }

    #[test]
    fn reload_changes_send_cei_and_activation_for_five_seconds() {
        let before = service_linking();
        let mut changed =
            parse_yaml(include_str!("../config.service-linking.example.yaml")).unwrap();
        changed.services[0].linking[0].links.pop(); // set-fu content changes
        changed.services[1].linking[0].active = false; // set-ri deactivated
        changed.frequencies.remove(1); // FM 0x1234 FI removed
        let after = changed.validate().unwrap();
        let events = DatabaseEvents::between(&before, &after);
        let mut carousel = FicCarousel::new().with_database_events(events, 0);
        let mut early = HashSet::new();
        let mut late = HashSet::new();
        for count in 0..400 {
            let clock = FrameClock::new(count, 1_704_164_645, 0).unwrap();
            for fig in figs(&carousel.write(&after, clock).unwrap()).concat() {
                if fig.kind == 0 && matches!(fig.extension, 6 | 21) {
                    for field in fields(&fig) {
                        let key = format!("{:02x}:{}", fig.bytes[1], hex(&field));
                        if count < database::EVENT_FRAMES {
                            early.insert(key)
                        } else {
                            late.insert(key)
                        };
                    }
                }
            }
        }
        // CEIs: short form with C/N = 0; set-fu's redefinition waits.
        assert!(early.contains("06:6abc") && !late.contains("06:6abc"));
        assert!(early.contains("15:0003123480"));
        assert!(
            !early.iter().any(|f| f.starts_with("06:eabc")) && late.contains("06:eabc028daa8daf")
        );
        // Activation state: short form with C/N = 1, then the definition.
        assert!(early.contains("86:1def") && !late.contains("86:1def"));
        assert!(
            late.contains("06:9def22ec8dab4f4c5d"),
            "LA = 0 in the definition"
        );
    }

    /// Move Radio One onto a named subchannel that a second service shares.
    fn share_with_second_service(config: &mut Config, language: u8) {
        config.services[0].components = vec![reference("audio_one")];
        let mut two = service(0x4da5, "Radio Two", vec![reference("audio_one")]);
        two.language = language;
        config.services.push(two);
        config.subchannels.insert(
            "audio_one".into(),
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
    }
}

/// Round trips through the vendored EDInburgh decoder, an independent reading of
/// EN 300 401, so that bit-layout mistakes in the writers cannot hide behind
/// tests derived from the writers themselves.
#[cfg(test)]
mod oracle_tests {
    use super::*;
    use crate::config::testing::{dab_plus, example, service};
    use crate::config::{ProtectionConfig, UserApplication};
    use crate::testsupport::edinburgh::fic::{DateTimeUTC, FicDecoder, Fig};
    use crate::testsupport::edinburgh::tables;
    use std::collections::{HashMap, HashSet};

    fn decode(fic: &[u8]) -> Vec<Fig> {
        for fib in fic.chunks(32) {
            assert_eq!(&fib[30..], &dabmux::edi::crc16(&fib[..30]).to_be_bytes());
        }
        FicDecoder::from_bytes(fic).unwrap()
    }

    fn twelve_service_config() -> ValidatedConfig {
        let mut config = example();
        config.ensemble.label = "Grüezi Mux".into();
        config.ensemble.short_label = Some("Grüezi".into());
        config.ensemble.international_table = 1;
        config.ensemble.local_time_offset_half_hours = 2;
        config.services.clear();
        for index in 0..12u8 {
            let (label, short_label) = if index == 0 {
                ("Radio Zürich 1".to_string(), Some("Zürich".to_string()))
            } else {
                (format!("Station {index}"), None)
            };
            let mut component = dab_plus(
                [72, 64, 48][usize::from(index / 4)],
                &format!("tcp://127.0.0.1:{}", 9001 + u16::from(index)),
            );
            component.subchannel_id = Some(index + 1);
            component.user_applications = vec![UserApplication::Slideshow];
            if index == 4 {
                component.protection = Some(ProtectionConfig::EepB { level: 2 });
            }
            let mut station = service(0x4001 + u32::from(index), &label, vec![component]);
            station.short_label = short_label;
            station.pty = 15;
            station.language = 8;
            config.services.push(station);
        }
        config.validate().unwrap()
    }

    #[test]
    fn decoder_sees_complete_and_correct_ensemble_description() {
        let valid = twelve_service_config();
        let source = &valid.source;
        let mut carousel = FicCarousel::new();
        let mut subchannels = HashMap::new();
        let mut components = HashMap::new();
        let mut languages = HashMap::new();
        let mut applications = HashMap::new();
        let mut labels = HashMap::new();
        let mut ensemble_label = None;
        let mut fig0_9_seen = false;
        for count in 0..100 {
            let fic = carousel
                .write(&valid, FrameClock::new(count, 0, 0).unwrap())
                .unwrap();
            let figs = decode(&fic);
            // Mode I: FIG 0/0 once per 96 ms transmission frame, first in FIB 0.
            let fig0_0 = figs
                .iter()
                .position(|fig| matches!(fig, Fig::F0_0(f) if f.eid == source.ensemble.id));
            assert_eq!(
                fig0_0,
                count.is_multiple_of(4).then_some(0),
                "FIG 0/0 placement in frame {count}"
            );
            for fig in figs {
                match fig {
                    Fig::F0_1(f) => {
                        for sub in f.subchannels {
                            subchannels.insert(sub.id, (sub.start, sub.size, sub.pl, sub.bitrate));
                        }
                    }
                    Fig::F0_2(f) => {
                        for c in f.services {
                            components.insert(c.sid, (c.tmid, c.scid, c.primary, c.ca));
                        }
                    }
                    Fig::F0_5(f) => {
                        for l in f.services {
                            languages.insert(l.scid, l.language);
                        }
                    }
                    Fig::F0_9(f) => {
                        assert_eq!((f.ecc, f.int_table_id, f.lto), (source.ensemble.ecc, 1, 1));
                        fig0_9_seen = true;
                    }
                    Fig::F0_13(f) => {
                        for s in f.services {
                            applications.insert(s.sid, (s.scids, s.uas));
                        }
                    }
                    Fig::F1_0(f) => ensemble_label = Some((f.eid, f.label, f.short_label)),
                    Fig::F1_1(f) => {
                        labels.insert(f.sid, (f.label, f.short_label));
                    }
                    _ => {}
                }
            }
        }

        assert!(fig0_9_seen);
        assert_eq!(
            ensemble_label,
            Some((source.ensemble.id, "Grüezi Mux".into(), "Grüezi".into()))
        );
        for (sub, raw) in valid.subchannels.iter().zip(&source.subchannels) {
            let expected_pl = match raw.protection {
                ProtectionConfig::EepA { level } => format!("EEP {level}-A"),
                ProtectionConfig::EepB { level } => format!("EEP {level}-B"),
            };
            assert_eq!(
                subchannels.get(&sub.id),
                Some(&(
                    usize::from(sub.start_address_cu),
                    Some(usize::from(sub.size_cu)),
                    Some(expected_pl),
                    Some(usize::from(sub.bitrate)),
                )),
                "FIG 0/1 for {}",
                sub.name
            );
        }
        let sids: HashSet<_> = source.services.iter().map(|s| s.id as u16).collect();
        for (service, sub) in source.services.iter().zip(&valid.subchannels) {
            let sid = service.id as u16;
            assert_eq!(components.get(&sid), Some(&(0, sub.id, true, false)));
            assert_eq!(languages.get(&sub.id), Some(&tables::Language::Deu));
            assert_eq!(
                applications.get(&sid),
                Some(&(0, vec![tables::UserApplication::Sls]))
            );
            let short: String = match &service.short_label {
                Some(short) => short.clone(),
                None => service.label.chars().take(8).collect(),
            };
            assert_eq!(labels.get(&sid), Some(&(service.label.clone(), short)));
        }
        assert_eq!(components.len(), sids.len());
    }

    #[test]
    fn decoder_reads_fig0_10_date_and_time() {
        let valid = twelve_service_config();
        let mut carousel = FicCarousel::new();
        // 2024-01-02 03:04:05.678 UTC, on a frame that carries FIG 0/10.
        let fic = carousel
            .write(&valid, FrameClock::new(4, 1_704_164_645, 678).unwrap())
            .unwrap();
        let time = decode(&fic)
            .into_iter()
            .find_map(|fig| match fig {
                Fig::F0_10(f) => Some(f),
                _ => None,
            })
            .expect("FIG 0/10 on frame phase 0");
        assert!(time.utc_flag && !time.lsi);
        match time.utc {
            DateTimeUTC::Long {
                year,
                month,
                day,
                hours,
                minutes,
                seconds,
                milliseconds,
            } => assert_eq!(
                (year, month, day, hours, minutes, seconds, milliseconds),
                (2024, 1, 2, 3, 4, 5, 678)
            ),
            DateTimeUTC::Short { .. } => panic!("expected long-form UTC"),
        }
    }
}
