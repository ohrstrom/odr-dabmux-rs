//! Live statistics for the web UI as server-sent events.
//!
//! Each client follows every frame and sends one event per interval
//! (`?interval_ms=`, default 250 ms, at least one frame of 24 ms). Buffer
//! levels come with their minimum and maximum over the interval, so that
//! brief underruns between two events still show.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tokio::time::{interval, Interval, MissedTickBehavior};

use crate::app::AppState;
use crate::runtime::{LiveFrame, LiveOutput, LiveSubchannel, RuntimeStats, StatsSnapshot};
use dabmux::frame::FRAME_PERIOD_MS;

const DEFAULT_INTERVAL_MS: u64 = 250;
const MAX_INTERVAL_MS: u64 = 10_000;

#[derive(Deserialize)]
pub struct StreamOptions {
    interval_ms: Option<u64>,
}

pub async fn stream(
    State(state): State<AppState>,
    Query(options): Query<StreamOptions>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let period = Duration::from_millis(
        options
            .interval_ms
            .unwrap_or(DEFAULT_INTERVAL_MS)
            .clamp(FRAME_PERIOD_MS, MAX_INTERVAL_MS),
    );
    let monitor = Monitor::new(state.stats.clone(), period);
    let events = stream::unfold(monitor, |mut monitor| async move {
        let event = monitor.next().await?;
        Some((Ok(event), monitor))
    });
    Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(5)))
}

/// One event: the latest frame, with buffer extremes over the interval.
#[derive(Serialize)]
struct Sample {
    frame: u64,
    unix_ms: i64,
    /// Frames seen since the previous event.
    frames: u64,
    /// Time since the previous event; with `frames`, the frame rate.
    elapsed_ms: f64,
    interval_ms: u64,
    counters: StatsSnapshot,
    subchannels: Vec<SubchannelSample>,
    outputs: Vec<LiveOutput>,
}

#[derive(Serialize)]
struct SubchannelSample {
    #[serde(flatten)]
    latest: LiveSubchannel,
    buffered_min: usize,
    buffered_max: usize,
}

struct Monitor {
    stats: Arc<RuntimeStats>,
    frames: watch::Receiver<Option<Arc<LiveFrame>>>,
    tick: Interval,
    period: Duration,
    latest: Option<Arc<LiveFrame>>,
    seen: u64,
    since: Instant,
    /// Buffer extremes per subchannel name since the last event.
    extremes: HashMap<String, (usize, usize)>,
}

impl Monitor {
    fn new(stats: Arc<RuntimeStats>, period: Duration) -> Self {
        let frames = stats.live.subscribe();
        let mut tick = interval(period);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        Self {
            stats,
            frames,
            tick,
            period,
            latest: None,
            seen: 0,
            since: Instant::now(),
            extremes: HashMap::new(),
        }
    }

    /// The next event; `None` ends the stream when the mux stops.
    async fn next(&mut self) -> Option<Event> {
        loop {
            tokio::select! {
                changed = self.frames.changed() => {
                    changed.ok()?;
                    let frame = self.frames.borrow_and_update().clone();
                    if let Some(frame) = frame {
                        self.record(frame);
                    }
                }
                _ = self.tick.tick() => {
                    if let Some(event) = self.event() {
                        return Some(event);
                    }
                }
            }
        }
    }

    fn record(&mut self, frame: Arc<LiveFrame>) {
        for sub in &frame.subchannels {
            self.extremes
                .entry(sub.name.clone())
                .and_modify(|(min, max)| {
                    *min = (*min).min(sub.buffered);
                    *max = (*max).max(sub.buffered);
                })
                .or_insert((sub.buffered, sub.buffered));
        }
        self.seen += 1;
        self.latest = Some(frame);
    }

    /// Nothing until the first frame arrives.
    fn event(&mut self) -> Option<Event> {
        let sample = self.sample()?;
        Event::default()
            .event("stats")
            .id(sample.frame.to_string())
            .json_data(sample)
            .ok()
    }

    /// The latest frame with the extremes since the previous sample.
    fn sample(&mut self) -> Option<Sample> {
        let frame = self.latest.clone()?;
        let subchannels = frame
            .subchannels
            .iter()
            .map(|sub| {
                let (min, max) = self
                    .extremes
                    .get(&sub.name)
                    .copied()
                    .unwrap_or((sub.buffered, sub.buffered));
                SubchannelSample {
                    latest: sub.clone(),
                    buffered_min: min,
                    buffered_max: max,
                }
            })
            .collect();
        let sample = Sample {
            frame: frame.frame,
            unix_ms: frame.unix_ms,
            frames: std::mem::take(&mut self.seen),
            elapsed_ms: std::mem::replace(&mut self.since, Instant::now())
                .elapsed()
                .as_secs_f64()
                * 1000.0,
            interval_ms: self.period.as_millis() as u64,
            counters: self.stats.snapshot(),
            subchannels,
            outputs: frame.outputs.clone(),
        };
        self.extremes.clear();
        Some(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::InputState;

    fn frame(frame: u64, buffered: usize) -> Arc<LiveFrame> {
        Arc::new(LiveFrame {
            frame,
            unix_ms: 0,
            subchannels: vec![LiveSubchannel {
                name: "4DA4/0".into(),
                id: 1,
                state: InputState::Receiving,
                buffered,
                capacity: 40,
                underflows: 0,
                drops: 0,
            }],
            outputs: Vec::new(),
        })
    }

    fn sample(monitor: &mut Monitor) -> serde_json::Value {
        serde_json::to_value(monitor.sample().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn events_carry_buffer_extremes_over_the_interval() {
        let mut monitor = Monitor::new(
            Arc::new(RuntimeStats::default()),
            Duration::from_millis(250),
        );
        assert!(monitor.sample().is_none(), "nothing before the first frame");
        for (n, buffered) in [12, 3, 20, 10].into_iter().enumerate() {
            monitor.record(frame(n as u64, buffered));
        }
        let first = sample(&mut monitor);
        assert_eq!(first["frames"], 4);
        let sub = &first["subchannels"][0];
        assert_eq!(
            (&sub["buffered"], &sub["buffered_min"], &sub["buffered_max"]),
            (&10.into(), &3.into(), &20.into())
        );

        monitor.record(frame(4, 11));
        let second = sample(&mut monitor);
        assert_eq!(second["frames"], 1);
        assert_eq!(second["subchannels"][0]["buffered_min"], 11);
    }
}
