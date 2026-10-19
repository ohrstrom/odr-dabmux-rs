//! FIC carousel (EN 300 401 clauses 5.2, 6 and 8).
//!
//! The FIC carries 90 bytes per 24 ms frame (120 in mode III). Above about
//! 20 services that is not enough for the MCI every 96 ms and all labels and
//! service information every second, so a [`FicPlan`] derived from the
//! configuration shares it out. The MCI keeps 96 ms where that fits;
//! otherwise it takes the shortest multiple of 96 ms, up to 384 ms, that
//! leaves the once-per-second FIGs their rate (clause 6.1 asks for 96 ms
//! "normally"). Only beyond that do the once-per-second FIGs slow down too.
//!
//! Each frame is filled in priority order:
//!
//! 1. FIG 0/0, then FIG 0/7, open the first FIB of each transmission frame
//!    (clause 6.4).
//! 2. FIG 0/10 once per second, and the labels (FIG 1/0, 1/1, 1/5), paced so
//!    that all of them take the planned label cycle, in the last FIB with
//!    room.
//! 3. Steady shares, from the last FIB backwards: FIG 0/9 with the FIG 0/8
//!    and 0/13 that may follow at the slower rate (clause 6.1), each within a
//!    second; and the service following databases (FIG 0/6, 0/21, 0/24),
//!    each entry within 10 s as TS 103 176 asks.
//! 4. One complete pass of the MCI (FIG 0/1, 0/2, 0/3, 0/14, and FIG 0/8 of
//!    secondary audio components) per planned MCI period.
//! 5. All remaining space carries FIG 0/5, 0/17 and 0/20, and change
//!    indications for five seconds after a reload, in rotation.
//!
//! The carousel measures how long each of these takes for a full cycle;
//! the runtime reports it in the stats.
//!
//! While a multiplex reconfiguration is announced (clause 6.5), FIG 0/0
//! carries the change flags and occurrence change, the MCI pass also carries
//! the next configuration's FIG 0/1, 0/2, 0/3, 0/7, 0/8 and 0/14 with C/N = 1
//! within twice the MCI period, and the slower share its FIG 0/8 and 0/13.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{bail, Result};
use serde::Serialize;

use crate::config::{
    encode_label, short_label_mask, SubchannelKind, UserApplication, ValidatedConfig,
};
use dabmux::frame::{cifs_per_transmission_frame, FrameClock, FRAME_PERIOD_MS};

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
/// Frames per nominal MCI repetition period (96 ms, clause 6.1).
const MCI_PERIOD_FRAMES: u64 = 4;
/// The longest MCI period the plan uses before it slows down the
/// once-per-second FIGs as well (384 ms).
const MAX_MCI_PERIOD_FRAMES: u64 = 16;
const FIB_DATA_BYTES: usize = 30;
/// Frames per FIG 0/10: once per second.
const TIME_FRAMES: u64 = 40;
/// Once-per-second information must repeat within 41 frames (984 ms).
const SECOND_FRAMES: u64 = 41;
/// Planned cycle of all labels, just under the required second.
const LABEL_CYCLE_FRAMES: u64 = 40;
/// Planned cycle of the once-per-second shares, with room for jitter.
const SHARE_CYCLE_FRAMES: u64 = 36;
/// Planned cycle of the service following databases (8 s), and their
/// limit (10 s, TS 103 176).
const DATABASE_CYCLE_FRAMES: u64 = 333;
const DATABASE_LIMIT_FRAMES: u64 = 417;
/// The database share takes at most this many bytes per frame.
const MAX_DATABASE_BYTES_PER_FRAME: usize = 8;
/// Share of the FIC the plan may fill, and the allowance for FIGs that do
/// not fit the end of a partly filled FIB.
const PLANNED_LOAD: f64 = 0.92;
const PACKING_OVERHEAD: f64 = 1.05;
/// The occurrence change holds the lower part of the CIF count, so it
/// addresses at most 249 CIFs ahead (clauses 6.4.1 and 6.5).
pub const MAX_ANNOUNCEMENT_FRAMES: u64 = 249;
/// C/N flag of the FIG type 0 header: the next configuration.
const NEXT: u8 = 0x80;

mod database;
pub use database::DatabaseEvents;

/// How a configuration shares the FIC. Durations are in frames and
/// reported in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FicPlan {
    /// FIC bytes per 24 ms frame.
    pub capacity_bytes: usize,
    /// Planned use of the capacity, in per mille.
    pub load_permille: u32,
    /// One complete MCI pass per period: 96 ms where it fits.
    #[serde(rename = "mci_period_ms", serialize_with = "frames_as_ms")]
    pub mci_period: u64,
    /// All labels once per cycle.
    #[serde(rename = "label_cycle_ms", serialize_with = "frames_as_ms")]
    pub label_cycle: u64,
    /// FIG 0/9 and the slower FIG 0/8 and 0/13 once per cycle; FIG 0/5,
    /// 0/17 and 0/20 rotate in the remaining space at least as often.
    #[serde(rename = "information_cycle_ms", serialize_with = "frames_as_ms")]
    pub information_cycle: u64,
    pub information_bytes_per_frame: usize,
    /// The service following databases once per cycle.
    #[serde(rename = "database_cycle_ms", serialize_with = "frames_as_ms")]
    pub database_cycle: u64,
    pub database_bytes_per_frame: usize,
    /// Whether labels and service information repeat within a second, as
    /// EN 300 401 requires.
    pub within_rates: bool,
}

fn frames_as_ms<S: serde::Serializer>(frames: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(frames * FRAME_PERIOD_MS)
}

impl FicPlan {
    pub fn new(config: &ValidatedConfig) -> Self {
        let ensemble = &config.source.ensemble;
        let capacity_bytes = fib_count(ensemble.mode) * FIB_DATA_BYTES;
        let per_frame = |bytes: usize, frames: u64| bytes as f64 * PACKING_OVERHEAD / frames as f64;
        let ensemble_information = 6 + if ensemble.reconfiguration_counter.is_some() {
            4
        } else {
            0
        };
        let fixed = per_frame(
            ensemble_information,
            cifs_per_transmission_frame(ensemble.mode),
        ) + per_frame(8, TIME_FRAMES);
        let clock = FrameClock::new(0, 0, 0).expect("frame zero");
        let mci = pass_bytes(&mci_groups(config));
        let labels = labels(config).1;
        let shared = pass_bytes(&share_groups(config, clock));
        let mut rotation = information_groups(config);
        rotation.extend(database::sci_groups(&config.databases));
        let rotation = pass_bytes(&rotation);
        let database = pass_bytes(&database::definition_groups(
            &config.databases,
            &database::Withheld::default(),
        ));
        let database_bytes_per_frame = if database == 0 {
            0
        } else {
            (per_frame(database, DATABASE_CYCLE_FRAMES).ceil() as usize)
                .clamp(1, MAX_DATABASE_BYTES_PER_FRAME)
        };
        let budget = capacity_bytes as f64 * PLANNED_LOAD - fixed - database_bytes_per_frame as f64;
        // What the once-per-second FIGs need at their nominal cycles.
        let second = per_frame(labels, LABEL_CYCLE_FRAMES)
            + per_frame(shared + rotation, SHARE_CYCLE_FRAMES);
        let mci_period = (1..=MAX_MCI_PERIOD_FRAMES / MCI_PERIOD_FRAMES)
            .map(|n| n * MCI_PERIOD_FRAMES)
            .find(|period| per_frame(mci, *period) + second <= budget)
            .unwrap_or(MAX_MCI_PERIOD_FRAMES);
        let room = budget - per_frame(mci, mci_period);
        // Beyond the longest MCI period, everything else slows down evenly.
        let stretch = if room > 0.0 {
            (second / room).clamp(1.0, 100.0)
        } else {
            100.0
        };
        let cycle = |frames: u64| (frames as f64 * stretch).ceil() as u64;
        let information_cycle = cycle(SHARE_CYCLE_FRAMES);
        let demand =
            fixed + database_bytes_per_frame as f64 + per_frame(mci, mci_period) + second / stretch;
        Self {
            capacity_bytes,
            load_permille: (demand / capacity_bytes as f64 * 1000.0).round() as u32,
            mci_period,
            label_cycle: cycle(LABEL_CYCLE_FRAMES),
            information_cycle,
            information_bytes_per_frame: per_frame(shared, information_cycle).ceil() as usize,
            database_cycle: if database_bytes_per_frame == 0 {
                0
            } else {
                (per_frame(database, 1) / database_bytes_per_frame as f64).ceil() as u64
            },
            database_bytes_per_frame,
            within_rates: stretch <= 1.0,
        }
    }

    /// The longest measured cycles that still meet the plan.
    fn label_limit(&self) -> u64 {
        (self.label_cycle + 1).max(SECOND_FRAMES)
    }

    fn information_limit(&self) -> u64 {
        self.information_cycle + SECOND_FRAMES - SHARE_CYCLE_FRAMES
    }

    fn database_limit(&self) -> u64 {
        self.database_cycle.max(DATABASE_LIMIT_FRAMES)
    }
}

/// The latest measured cycle of each part of the FIC, in frames: from one
/// completed pass to the next.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FicCycles {
    pub mci: Option<u64>,
    pub labels: Option<u64>,
    /// FIG 0/9 and the slower FIG 0/8 and 0/13.
    pub shared: Option<u64>,
    /// FIG 0/5, 0/17 and 0/20.
    pub rotation: Option<u64>,
    pub databases: Option<u64>,
}

#[derive(Debug, Default)]
struct Cycle {
    since: Option<u64>,
}

impl Cycle {
    /// Record a completed pass in frame `count`; counts a cycle longer than
    /// `limit` frames as late.
    fn complete(&mut self, count: u64, limit: u64, last: &mut Option<u64>, late: &mut u64) {
        if let Some(since) = self.since {
            let cycle = (count - since).max(1);
            if cycle > limit {
                *late += 1;
            }
            *last = Some(cycle);
        }
        self.since = Some(count);
    }
}

pub struct FicCarousel {
    /// Derived from the configuration on the first frame.
    plan: Option<FicPlan>,
    /// First frame this carousel wrote.
    started: Option<u64>,
    mci: Cursor,
    /// First frame of the MCI repetition period whose pass has been sent.
    mci_sent_for: Option<u64>,
    /// The MCI repetition period under way, and its length in frames.
    mci_period: Option<(u64, u64)>,
    time_due: bool,
    label_cursor: usize,
    /// Labels due, in units of one label cycle per label.
    label_credit: u64,
    shared: Cursor,
    shared_credit: usize,
    rotation: Cursor,
    database: Cursor,
    database_credit: usize,
    cycles: FicCycles,
    measured: [Cycle; 5],
    /// Cycles that took longer than planned, not yet reported.
    late: u64,
    events: DatabaseEvents,
    /// Frame count until which `events` are signalled.
    events_until: u64,
    reconfiguration_counter: Option<u16>,
    announced: Option<Announced>,
}

/// The next configuration, encoded once when announced.
struct Announced {
    /// Frame count of the first CIF of the next configuration.
    at: u64,
    mci: Vec<Group>,
    /// FIG 0/8 and 0/13, which may follow at the slower rate (clause 6.1).
    applications: Vec<Group>,
}

impl Default for FicCarousel {
    fn default() -> Self {
        Self::new()
    }
}

impl FicCarousel {
    pub fn new() -> Self {
        Self {
            plan: None,
            started: None,
            mci: Cursor::default(),
            mci_sent_for: None,
            mci_period: None,
            time_due: true,
            label_cursor: 0,
            label_credit: u64::MAX,
            shared: Cursor::default(),
            shared_credit: 0,
            rotation: Cursor::default(),
            database: Cursor::default(),
            database_credit: 0,
            cycles: FicCycles::default(),
            measured: Default::default(),
            late: 0,
            events: DatabaseEvents::default(),
            events_until: 0,
            reconfiguration_counter: None,
            announced: None,
        }
    }

    /// Announce that `next` replaces the configuration from frame `at`,
    /// signalling `counter` in its FIG 0/7. The announcement starts once `at`
    /// is within reach of the occurrence change. The runtime replaces the
    /// carousel when it switches.
    pub fn announce(&mut self, next: &ValidatedConfig, at: u64, counter: Option<u16>) {
        let mut mci = mci_groups(next);
        if let Some(counter) = counter {
            let entry = fig0_7_entry(counter, next.source.services.len() as u8);
            mci.push(Group::new(7, false, vec![entry.to_vec()]));
        }
        let applications = application_groups(next);
        let flag = |mut groups: Vec<Group>| {
            for group in &mut groups {
                group.extension |= NEXT;
            }
            groups
        };
        self.announced = Some(Announced {
            at,
            mci: flag(mci),
            applications: flag(applications),
        });
    }

    /// Stop announcing a reconfiguration that will not take place.
    pub fn withdraw(&mut self) {
        self.announced = None;
    }

    /// The announcement whose occurrence change is within reach at `count`.
    fn announcing(&self, count: u64) -> Option<&Announced> {
        self.announced
            .as_ref()
            .filter(|next| next.at > count && next.at - count <= MAX_ANNOUNCEMENT_FRAMES)
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

    /// The latest measured cycles.
    pub fn cycles(&self) -> FicCycles {
        self.cycles
    }

    /// Cycles that took longer than planned since the last call: an MCI
    /// period without a complete pass, or a later label, information or
    /// database cycle.
    pub fn take_late_cycles(&mut self) -> u64 {
        std::mem::take(&mut self.late)
    }

    pub fn write(&mut self, config: &ValidatedConfig, clock: FrameClock) -> Result<Vec<u8>> {
        let plan = *self.plan.get_or_insert_with(|| FicPlan::new(config));
        let started = *self.started.get_or_insert(clock.count);
        let mode = config.source.ensemble.mode;
        let mut fibs = vec![Fib::default(); fib_count(mode)];

        if clock
            .count
            .is_multiple_of(cifs_per_transmission_frame(mode))
        {
            let next_at = self.announcing(clock.count).map(|next| next.at);
            fibs[0].push(&fig0_0(config.source.ensemble.id, clock.count, next_at))?;
            let counter = self
                .reconfiguration_counter
                .or(config.source.ensemble.reconfiguration_counter);
            if let Some(counter) = counter {
                fibs[0].push(&fig0_7(counter, config.source.services.len() as u8))?;
            }
        }

        // Small, timed FIGs first.
        if clock.count.is_multiple_of(TIME_FRAMES) {
            self.time_due = true;
        }
        if self.time_due {
            if let Some(fib) = fibs.iter_mut().find(|fib| fib.room() >= 8) {
                fib.push(&fig0_10(clock))?;
                self.time_due = false;
            }
        }
        // Each frame earns one label cycle's worth of credit per label, so
        // all labels take the planned cycle; sending them more often only
        // takes space from the rest.
        let (labels, _) = labels(config);
        self.label_credit = self
            .label_credit
            .saturating_add(labels as u64)
            .min(plan.label_cycle + labels as u64);
        while self.label_credit >= plan.label_cycle {
            let label = self.label(config);
            let Some(fib) = fibs.iter_mut().rev().find(|fib| fib.room() >= label.len()) else {
                break;
            };
            fib.push(&label)?;
            self.label_credit -= plan.label_cycle;
            self.label_cursor = (self.label_cursor + 1) % labels;
            if self.label_cursor == 0 {
                self.measured[1].complete(
                    clock.count,
                    plan.label_limit(),
                    &mut self.cycles.labels,
                    &mut self.late,
                );
            }
        }

        // Steady shares, before the MCI takes what it needs of the rest.
        let mut shared = share_groups(config, clock);
        let mut shared_bytes = plan.information_bytes_per_frame;
        if let Some(next) = self.announcing(clock.count) {
            shared.extend(next.applications.iter().cloned());
            shared_bytes *= 2;
        }
        if fill_share(
            &mut fibs,
            &shared,
            &mut self.shared,
            &mut self.shared_credit,
            shared_bytes,
        )? {
            self.measured[2].complete(
                clock.count,
                plan.information_limit(),
                &mut self.cycles.shared,
                &mut self.late,
            );
        }
        let events_active = clock.count < self.events_until && !self.events.is_empty();
        let no_events = database::Withheld::default();
        let withheld = if events_active {
            self.events.withheld()
        } else {
            &no_events
        };
        let definitions = database::definition_groups(&config.databases, withheld);
        if fill_share(
            &mut fibs,
            &definitions,
            &mut self.database,
            &mut self.database_credit,
            plan.database_bytes_per_frame,
        )? {
            self.measured[4].complete(
                clock.count,
                plan.database_limit(),
                &mut self.cycles.databases,
                &mut self.late,
            );
        }

        // While a reconfiguration is announced, the current and the next
        // MCI share twice the period (clause 6.1).
        let period_frames = if self.announcing(clock.count).is_some() {
            2 * plan.mci_period
        } else {
            plan.mci_period
        };
        let period = clock.count / period_frames * period_frames;
        if self.mci_period != Some((period, period_frames)) {
            if let Some((previous, frames)) = self.mci_period {
                let measured = frames == period_frames && previous >= started;
                if measured && self.mci_sent_for != Some(previous) {
                    self.late += 1;
                }
            }
            self.mci_period = Some((period, period_frames));
        }
        if self.mci_sent_for != Some(period) {
            // Next after current, so that a pass under way when the
            // announcement starts keeps its place.
            let mut groups = mci_groups(config);
            if let Some(next) = self.announcing(clock.count) {
                groups.extend(next.mci.iter().cloned());
            }
            for fib in &mut fibs {
                if fill(fib, &groups, &mut self.mci, Fill::OnePass)? {
                    self.mci_sent_for = Some(period);
                    self.measured[0].complete(
                        clock.count,
                        u64::MAX,
                        &mut self.cycles.mci,
                        &mut self.late,
                    );
                    break;
                }
            }
        }

        let mut rotation = information_groups(config);
        rotation.extend(database::sci_groups(&config.databases));
        if events_active {
            rotation.extend(self.events.groups().iter().cloned());
        }
        if has_entries(&rotation) {
            let mut ended = false;
            for fib in &mut fibs {
                ended |= fill(fib, &rotation, &mut self.rotation, Fill::Repeat)?;
            }
            if ended {
                self.measured[3].complete(
                    clock.count,
                    plan.information_limit(),
                    &mut self.cycles.rotation,
                    &mut self.late,
                );
            }
        }

        let mut output = Vec::with_capacity(fibs.len() * 32);
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

fn fib_count(mode: u8) -> usize {
    if mode == 3 {
        4
    } else {
        3
    }
}

/// The number of labels, and their bytes: the ensemble's, the services',
/// and those of services announced in other ensembles. A FIG 1 label is
/// 22 bytes with a 16-bit and 24 with a 32-bit identifier.
fn labels(config: &ValidatedConfig) -> (usize, usize) {
    let size = |data: bool| if data { 24 } else { 22 };
    let services = config.services.iter().map(|service| size(service.data));
    let announced = config.databases.labels.iter().map(|label| size(label.data));
    let bytes = 22 + services.chain(announced).sum::<usize>();
    (
        1 + config.services.len() + config.databases.labels.len(),
        bytes,
    )
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

fn has_entries(groups: &[Group]) -> bool {
    groups.iter().any(|group| !group.entries.is_empty())
}

/// The bytes of one pass of `groups` packed into empty FIBs: the entries
/// plus a two-byte header per FIG.
fn pass_bytes(groups: &[Group]) -> usize {
    let mut total = 0;
    for group in groups {
        let mut fig = 0;
        for entry in &group.entries {
            if fig > 0 && fig + entry.len() > FIB_DATA_BYTES {
                total += fig;
                fig = 0;
            }
            if fig == 0 {
                fig = 2;
            }
            fig += entry.len();
        }
        total += fig;
    }
    total
}

/// Fill `groups` from the last FIB backwards, using at most the credit that
/// `per_frame` bytes a frame have earned. Credit accumulates while unused,
/// up to a FIB or two frames' worth, so that large entries fit. Returns
/// whether a pass ended.
fn fill_share(
    fibs: &mut [Fib],
    groups: &[Group],
    cursor: &mut Cursor,
    credit: &mut usize,
    per_frame: usize,
) -> Result<bool> {
    if !has_entries(groups) {
        return Ok(false);
    }
    *credit = (*credit + per_frame).min(FIB_DATA_BYTES.max(2 * per_frame));
    let mut ended = false;
    for fib in fibs.iter_mut().rev() {
        let before = fib.used;
        fib.cap = (fib.used + *credit).min(FIB_DATA_BYTES);
        ended |= fill(fib, groups, cursor, Fill::Repeat)?;
        fib.cap = FIB_DATA_BYTES;
        *credit -= fib.used - before;
    }
    Ok(ended)
}

/// Pack entries from `cursor` into `fib` until it is full, or the pass ends
/// for `Fill::OnePass`. Returns whether the end of the pass was reached.
fn fill(fib: &mut Fib, groups: &[Group], cursor: &mut Cursor, mode: Fill) -> Result<bool> {
    if !has_entries(groups) {
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
            return Ok(true);
        }
        let group = &groups[cursor.group];
        let first = &group.entries[cursor.entry];
        if first.len() + 2 > FIB_DATA_BYTES {
            bail!("FIG 0/{} entry does not fit a FIB", group.extension & 0x1f);
        }
        if fib.room() < first.len() + 2 {
            return Ok(wrapped);
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
    let ([programme_definitions, data_definitions], _) = component_definitions(config);
    vec![
        Group::new(1, false, subchannels),
        Group::new(2, false, programme),
        Group::new(2, true, data),
        Group::new(3, false, packet_components),
        Group::new(8, false, programme_definitions),
        Group::new(8, true, data_definitions),
        Group::new(14, false, fec),
    ]
}

/// Whether replacing `active` with `candidate` is a multiplex
/// reconfiguration: a change to the ensemble identity or to any MCI FIG
/// (clause 6.1). Such a change has to be announced (clause 6.5).
pub fn reconfigures(active: &ValidatedConfig, candidate: &ValidatedConfig) -> bool {
    active.source.ensemble.id != candidate.source.ensemble.id
        || mci_groups(active) != mci_groups(candidate)
        || application_groups(active) != application_groups(candidate)
}

/// The once-per-second share: FIG 0/9, then the FIG 0/13 and 0/8 that may
/// follow at the slower rate.
fn share_groups(config: &ValidatedConfig, clock: FrameClock) -> Vec<Group> {
    let mut groups = vec![Group::new(9, false, vec![fig0_9_entry(config, clock)])];
    groups.extend(application_groups(config));
    groups
}

/// FIG 0/5 and 0/17, for the rotation.
fn information_groups(config: &ValidatedConfig) -> Vec<Group> {
    let source = &config.source;
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
    vec![
        Group::new(5, false, languages),
        Group::new(17, false, programme_types),
    ]
}

fn sid_bytes(config: &ValidatedConfig, service: usize) -> Vec<u8> {
    let id = config.source.services[service].id;
    if config.services[service].data {
        id.to_be_bytes().to_vec()
    } else {
        (id as u16).to_be_bytes().to_vec()
    }
}

/// FIG 0/13 and the FIG 0/8 of [`component_definitions`] that may follow
/// at the slower rate, each split by P/D: the part of the MCI that may be
/// signalled less often than every 96 ms (clause 6.1).
fn application_groups(config: &ValidatedConfig) -> Vec<Group> {
    let source = &config.source;
    let mut applications = [Vec::new(), Vec::new()];
    for (component, raw) in config.components.iter().zip(&source.components) {
        let data = config.services[component.service].data;
        let audio = source.subchannels[component.subchannel].kind.is_audio();
        if !raw.user_applications.is_empty() {
            let mut entry = sid_bytes(config, component.service);
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
    }
    let [programme_applications, data_applications] = applications;
    let (_, [programme_definitions, data_definitions]) = component_definitions(config);
    vec![
        Group::new(13, false, programme_applications),
        Group::new(13, true, data_applications),
        Group::new(8, false, programme_definitions),
        Group::new(8, true, data_definitions),
    ]
}

/// FIG 0/8 entries, by P/D, for the components that need one: secondary
/// components and components with user applications (clause 6.3.5). The
/// first pair, for secondary audio components, belongs to the 96 ms MCI;
/// the second, for primary components with data applications and data
/// secondary components, may follow at the slower rate (clause 6.1).
type Definitions = [Vec<Vec<u8>>; 2];

fn component_definitions(config: &ValidatedConfig) -> (Definitions, Definitions) {
    let source = &config.source;
    let mut mci: Definitions = Default::default();
    let mut slower: Definitions = Default::default();
    for (component, raw) in config.components.iter().zip(&source.components) {
        if component.scids == 0 && raw.user_applications.is_empty() {
            continue;
        }
        let mut entry = sid_bytes(config, component.service);
        entry.push(component.scids & 0x0f);
        match component.scid {
            Some(scid) => entry.extend_from_slice(&[0x80 | (scid >> 8) as u8, scid as u8]),
            None => entry.push(config.subchannels[component.subchannel].id & 0x3f),
        }
        let data = usize::from(config.services[component.service].data);
        let audio = source.subchannels[component.subchannel].kind.is_audio();
        if component.scids > 0 && audio {
            mci[data].push(entry);
        } else {
            slower[data].push(entry);
        }
    }
    (mci, slower)
}

/// FIG 0/0; `next_at` announces the frame count from which the next
/// configuration applies, with Change flags 11 and the occurrence change.
fn fig0_0(eid: u16, count: u64, next_at: Option<u64>) -> Vec<u8> {
    let high = ((count / 250) % 20) as u8;
    let low = (count % 250) as u8;
    let [e0, e1] = eid.to_be_bytes();
    match next_at {
        None => vec![5, 0, e0, e1, high, low],
        Some(at) => vec![6, 0, e0, e1, 0xc0 | high, low, (at % 250) as u8],
    }
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
    let [high, low] = fig0_7_entry(counter, service_count);
    [3, 7, high, low]
}

fn fig0_7_entry(counter: u16, service_count: u8) -> [u8; 2] {
    [
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
        // C/N = 1 on MCI marks the next configuration (clause 6.1).
        if fig.kind == 0 && fig.bytes[1] & NEXT != 0 && MCI_EXTENSIONS.contains(&fig.extension) {
            keys = keys.into_iter().map(|key| format!("next {key}")).collect();
        }
        keys
    }

    const MCI_EXTENSIONS: [u8; 7] = [1, 2, 3, 7, 8, 13, 14];

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
        mux_zh_source().validate().unwrap()
    }

    fn mux_zh_source() -> Config {
        parse_yaml(include_str!("../config.mux-zh.example.yaml")).unwrap()
    }

    /// mux-zh before and after its first service drops from 72 to 64 kbit/s.
    fn mux_zh_bitrate_change() -> (ValidatedConfig, ValidatedConfig) {
        let mut next = mux_zh_source();
        next.services[0].components[0].bitrate = Some(64);
        (mux_zh(), next.validate().unwrap())
    }

    /// FIGs of the frames from `count` 0 while `next`, from frame `at`, is
    /// announced.
    fn run_announced(
        config: &ValidatedConfig,
        next: &ValidatedConfig,
        at: u64,
        frames: u64,
    ) -> Vec<Vec<Fig>> {
        let mut carousel = FicCarousel::new().with_reconfiguration_counter(Some(7));
        carousel.announce(next, at, Some(8));
        (0..frames)
            .map(|count| {
                let clock =
                    FrameClock::new(count, 1_704_164_645, (count % 41) as u16 * 24).unwrap();
                figs(&carousel.write(config, clock).unwrap()).concat()
            })
            .collect()
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
            let audio = config.source.subchannels[component.subchannel]
                .kind
                .is_audio();
            if component.scids > 0 && audio {
                let sid = hex(&sid_bytes(config, component.service));
                keys.insert(format!("0/8 {sid}/{}", component.scids));
            }
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

    /// `count` DAB+ services with slideshow, PTY and language, at `bitrate`.
    fn ensemble(count: usize, bitrate: u16) -> ValidatedConfig {
        let mut config = example();
        config.services.clear();
        for index in 0..count {
            let mut component = dab_plus(bitrate, &format!("tcp://:{}", 20000 + index));
            component.subchannel_id = Some(index as u8 + 1);
            component.user_applications = vec![UserApplication::Slideshow];
            let label = format!("Station {index:02}");
            let mut station = service(0xc100 + index as u32, &label, vec![component]);
            station.pty = 10;
            station.language = 9;
            config.services.push(station);
        }
        config.validate().unwrap()
    }

    /// `example()` with a second audio component, whose FIG 0/8 is MCI.
    fn secondary_audio() -> ValidatedConfig {
        let mut config = example();
        let mut second = dab_plus(64, "udp://127.0.0.1:9002");
        second.subchannel_id = Some(2);
        config.services[0].components.push(second);
        config.validate().unwrap()
    }

    #[test]
    fn mci_is_complete_in_every_planned_period() {
        for (config, period) in [
            (example().validate().unwrap(), 4),
            (secondary_audio(), 4),
            (mux_zh(), 4),
            (ensemble(30, 32), 8),
            (ensemble(50, 16), 16),
        ] {
            assert_eq!(FicPlan::new(&config).mci_period, period);
            let expected = expected_mci(&config);
            for (index, frames) in run(&config, 800).chunks(period as usize).enumerate() {
                let sent: HashSet<String> = frames.iter().flatten().flat_map(entries).collect();
                let missing: Vec<_> = expected.difference(&sent).collect();
                assert!(missing.is_empty(), "period {index} misses {missing:?}");
            }
        }
    }

    #[test]
    fn plan_relaxes_the_mci_before_the_once_per_second_information() {
        let plans: Vec<_> = [(12, 64), (24, 32), (30, 32), (40, 16), (50, 16)]
            .into_iter()
            .map(|(count, bitrate)| {
                let plan = FicPlan::new(&ensemble(count, bitrate));
                (plan.mci_period, plan.label_cycle, plan.within_rates)
            })
            .collect();
        assert_eq!(
            plans,
            [
                (4, 40, true),
                (8, 40, true),
                (8, 40, true),
                (12, 40, true),
                (16, 46, false)
            ]
        );
        let overloaded = ensemble(50, 16).warnings();
        assert!(overloaded
            .iter()
            .any(|w| w.contains("labels every 1104 ms")));
        assert!(ensemble(40, 16).warnings().is_empty());
        let resolved = serde_json::to_value(ensemble(30, 32).resolved()).unwrap();
        assert_eq!(resolved["fic"]["mci_period_ms"], 192);
        assert_eq!(resolved["fic"]["label_cycle_ms"], 960);
    }

    #[test]
    fn large_ensembles_meet_their_plan() {
        for config in [ensemble(30, 32), ensemble(50, 16)] {
            let plan = FicPlan::new(&config);
            let mut carousel = FicCarousel::new();
            let mut seen: HashMap<String, Vec<u64>> = HashMap::new();
            for count in 0..2000 {
                let clock = FrameClock::new(count, 1_704_164_645, 0).unwrap();
                for fig in figs(&carousel.write(&config, clock).unwrap()).concat() {
                    for key in entries(&fig) {
                        seen.entry(key).or_default().push(count);
                    }
                }
                assert_eq!(
                    carousel.take_late_cycles(),
                    0,
                    "late cycle in frame {count}"
                );
            }
            for (key, frames) in &seen {
                let limit = match key.split(' ').next().unwrap() {
                    "0/0" | "0/1" | "0/2" => plan.mci_period + 1,
                    "1/0" | "1/1" => plan.label_limit(),
                    "0/10" => TIME_FRAMES,
                    _ => plan.information_limit(),
                };
                let gaps =
                    std::iter::once(frames[0] + 1).chain(frames.windows(2).map(|w| w[1] - w[0]));
                let worst = gaps.max().unwrap();
                assert!(worst <= limit, "{key} repeats only every {worst} frames");
            }
            let cycles = carousel.cycles();
            // Passes end anywhere in their period, so up to two periods apart.
            assert!(cycles.mci.is_some_and(|cycle| cycle < 2 * plan.mci_period));
            assert_eq!(cycles.labels, Some(plan.label_cycle));
            assert!(cycles.shared.unwrap() <= plan.information_limit());
        }
    }

    #[test]
    fn time_is_sent_once_per_second() {
        let frames = run(&mux_zh(), 400);
        let sent: Vec<usize> = (0..frames.len())
            .filter(|&index| {
                frames[index]
                    .iter()
                    .any(|fig| (fig.kind, fig.extension) == (0, 10))
            })
            .collect();
        assert_eq!(sent, (0..400).step_by(40).collect::<Vec<_>>());
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
    fn announcement_sets_change_flags_and_occurrence_change() {
        let (active, next) = mux_zh_bitrate_change();
        // The occurrence change reaches 249 CIFs: frames before 151 do not
        // announce a switch at 400 yet.
        let frames = run_announced(&active, &next, 400, 420);
        for (count, figs) in frames.iter().enumerate() {
            let count = count as u64;
            let fig0_0 = figs.iter().find(|fig| fig.kind == 0 && fig.extension == 0);
            assert_eq!(fig0_0.is_some(), count.is_multiple_of(4), "frame {count}");
            let Some(fig0_0) = fig0_0 else { continue };
            let body = fig0_0.body();
            assert_eq!(
                (u16::from(body[2] & 0x1f) * 250 + u16::from(body[3])) as u64,
                count % 5000
            );
            if (151..400).contains(&count) {
                assert_eq!(body[2] >> 6, 0b11, "change flags in frame {count}");
                assert_eq!(body[4..], [(400 % 250) as u8], "occurrence change");
            } else {
                assert_eq!(body[2] >> 6, 0, "change flags in frame {count}");
                assert_eq!(body.len(), 4, "no occurrence change in frame {count}");
            }
        }
        for (count, figs) in frames.iter().enumerate() {
            let next_mci = figs
                .iter()
                .flat_map(entries)
                .any(|key| key.starts_with("next "));
            assert!(
                !next_mci || (151..400).contains(&count),
                "next MCI in frame {count}"
            );
        }
        assert!(frames[151..159]
            .iter()
            .flatten()
            .flat_map(entries)
            .any(|key| key.starts_with("next ")));
    }

    #[test]
    fn announced_and_current_mci_complete_within_two_96_ms_periods() {
        let (active, next) = mux_zh_bitrate_change();
        let frames = run_announced(&active, &next, 240, 240);
        let current = expected_mci(&active);
        let mut announced: HashSet<String> = expected_mci(&next)
            .into_iter()
            .map(|key| format!("next {key}"))
            .collect();
        announced.insert("next 0/7".into());
        // Clause 6.1: during the reconfiguration, the next MCI within two
        // consecutive 96 ms periods; the current once in that time.
        for (period, window) in frames.chunks(8).enumerate() {
            let sent: HashSet<String> = window.iter().flatten().flat_map(entries).collect();
            for expected in [&current, &announced] {
                let missing: Vec<_> = expected.difference(&sent).collect();
                assert!(
                    missing.is_empty(),
                    "192 ms period {period} misses {missing:?}"
                );
            }
        }
    }

    #[test]
    fn information_and_next_applications_repeat_once_per_second_while_announced() {
        let (active, next) = mux_zh_bitrate_change();
        let frames = run_announced(&active, &next, 240, 240);
        let mut seen: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, figs) in frames.iter().enumerate() {
            for key in figs.iter().flat_map(entries) {
                seen.entry(key).or_default().push(index);
            }
        }
        let count = |prefix: &str| seen.keys().filter(|key| key.starts_with(prefix)).count();
        assert_eq!(
            [
                count("0/13 "),
                count("0/8 "),
                count("next 0/13 "),
                count("next 0/8 ")
            ],
            [18, 18, 18, 18]
        );
        for (key, frames) in &seen {
            let gaps = std::iter::once(frames[0] + 1).chain(frames.windows(2).map(|w| w[1] - w[0]));
            let worst = gaps.max().unwrap();
            assert!(worst <= 41, "{key} repeats only every {worst} frames");
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
        let config = config.validate().unwrap();
        let sent: HashSet<Vec<u8>> = run(&config, 41)
            .iter()
            .flatten()
            .map(|fig| fig.bytes.clone())
            .collect();
        assert!(
            sent.contains(&vec![5, 8, 0x4d, 0xa4, 1, 2]),
            "FIG 0/8 short form, SCIdS 1"
        );
        // A secondary audio component: MCI, not the slower rate (clause 6.1).
        assert!(mci_groups(&config)
            .iter()
            .any(|group| group.extension == 8 && group.entries == [vec![0x4d, 0xa4, 1, 2]]));
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
    use crate::config::{Config, ProtectionConfig, UserApplication};
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
        twelve_service_source().validate().unwrap()
    }

    fn twelve_service_source() -> Config {
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
        config
    }

    #[test]
    fn decoder_reads_the_announced_reconfiguration() {
        let mut next = twelve_service_source();
        next.services[0].components[0].bitrate = Some(64);
        let (active, next) = (twelve_service_config(), next.validate().unwrap());
        let first = next.subchannels[0].id;
        let mut carousel = FicCarousel::new();
        carousel.announce(&next, 1240, None);
        let mut sizes = HashMap::new();
        for count in 1000..1100 {
            for fig in decode(
                &carousel
                    .write(&active, FrameClock::new(count, 0, 0).unwrap())
                    .unwrap(),
            ) {
                match fig {
                    Fig::F0_0(f) => {
                        assert_eq!(f.cif_count, count as u16);
                        assert_eq!((f.change_flags, f.occurrence_change), (3, Some(240)));
                    }
                    Fig::F0_1(f) => {
                        for sub in f.subchannels.iter().filter(|sub| sub.id == first) {
                            sizes.insert(f.base.cn, sub.bitrate);
                        }
                    }
                    _ => {}
                }
            }
        }
        assert_eq!(sizes, HashMap::from([(false, Some(72)), (true, Some(64))]));
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
