//! Timezone and TAI–UTC data used by the on-air clock.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use chrono::{Local, Offset, TimeZone, Utc};
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};

use crate::config::EnsembleConfig;

const NTP_UNIX_EPOCH_DELTA: i64 = 2_208_988_800;

#[derive(Clone, PartialEq, Eq)]
pub enum TaiSource {
    Disabled,
    Fixed(u8),
    Bulletins(Vec<String>),
}

impl TaiSource {
    pub fn from_config(config: &EnsembleConfig) -> Self {
        if !config.tist {
            Self::Disabled
        } else if let Some(offset) = config.tai_utc_offset {
            Self::Fixed(offset)
        } else {
            Self::Bulletins(config.tai_clock_bulletins.clone())
        }
    }

    pub async fn resolve(&self) -> Result<u8> {
        match self {
            Self::Disabled => Ok(0),
            Self::Fixed(offset) => Ok(*offset),
            Self::Bulletins(urls) => fetch_bulletin_offset(urls).await,
        }
    }
}

pub struct TaiClock {
    pub source: TaiSource,
    offset: Arc<AtomicU8>,
    refresh_task: Option<JoinHandle<()>>,
}

impl TaiClock {
    pub fn new(source: TaiSource, initial: u8) -> Self {
        let offset = Arc::new(AtomicU8::new(initial));
        let refresh_task = if let TaiSource::Bulletins(urls) = &source {
            let urls = urls.clone();
            let shared = offset.clone();
            Some(tokio::spawn(async move {
                let mut interval = time::interval(Duration::from_secs(3600));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    match fetch_bulletin_offset(&urls).await {
                        Ok(value) => {
                            shared.store(value, Ordering::Relaxed);
                            tracing::info!(tai_utc_offset = value, "TAI bulletin refreshed");
                        }
                        Err(err) => tracing::warn!(%err, "TAI bulletin refresh failed"),
                    }
                }
            }))
        } else {
            None
        };
        Self {
            source,
            offset,
            refresh_task,
        }
    }

    pub fn offset(&self) -> u8 {
        self.offset.load(Ordering::Relaxed)
    }
}

impl Drop for TaiClock {
    fn drop(&mut self) {
        if let Some(task) = &self.refresh_task {
            task.abort();
        }
    }
}

/// Host timezone offset for FIG 0/9, which can only express half hours.
/// Offsets such as +5:45 (Nepal) truncate towards zero, to +5:30.
pub fn local_offset_half_hours(unix_seconds: i64) -> Result<i8> {
    let utc = Utc
        .timestamp_opt(unix_seconds, 0)
        .single()
        .context("timestamp outside local-time range")?;
    let seconds = utc.with_timezone(&Local).offset().fix().local_minus_utc();
    i8::try_from(seconds / 1800).context("local time offset exceeds FIG 0/9 range")
}

#[cfg(test)]
fn parse_ietf_bulletin(text: &str, now_unix: i64) -> Result<u8> {
    let (offset, expired) = parse_ietf_bulletin_any(text, now_unix)?;
    if expired {
        bail!("TAI bulletin has expired");
    }
    Ok(offset)
}

fn parse_ietf_bulletin_any(text: &str, now_unix: i64) -> Result<(u8, bool)> {
    let mut expires = None;
    let mut latest = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#@") {
            let ntp: i64 = rest
                .split_whitespace()
                .next()
                .context("missing bulletin expiry")?
                .parse()?;
            expires = Some(ntp - NTP_UNIX_EPOCH_DELTA);
            continue;
        }
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(first) = fields.next() else { continue };
        let Some(second) = fields.next() else {
            continue;
        };
        let (Ok(ntp), Ok(offset)) = (first.parse::<i64>(), second.parse::<u8>()) else {
            continue;
        };
        let effective = ntp - NTP_UNIX_EPOCH_DELTA;
        if effective <= now_unix && latest.is_none_or(|(old, _)| effective > old) {
            latest = Some((effective, offset));
        }
    }
    let expiry = expires.context("TAI bulletin has no #@ expiry date")?;
    let (_, offset) = latest.context("TAI bulletin has no current offset")?;
    if !(32..=255).contains(&offset) {
        bail!("TAI bulletin offset is outside the supported range");
    }
    Ok((offset, expiry <= now_unix))
}

async fn fetch_bulletin_offset(urls: &[String]) -> Result<u8> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let now = chrono::Utc::now().timestamp();
    let mut errors = Vec::new();
    let mut expired_fallback = None;
    for url in urls {
        let result = async {
            let response = client.get(url).send().await?.error_for_status()?;
            let text = response.text().await?;
            parse_ietf_bulletin_any(&text, now)
        }
        .await;
        match result {
            Ok((offset, false)) => return Ok(offset),
            Ok((offset, true)) => {
                tracing::warn!(%url, "TAI bulletin expired; checking alternatives");
                expired_fallback.get_or_insert(offset);
            }
            Err(err) => errors.push(format!("{url}: {err}")),
        }
    }
    if let Some(offset) = expired_fallback {
        tracing::warn!(
            tai_utc_offset = offset,
            "using expired TAI bulletin as fallback"
        );
        return Ok(offset);
    }
    bail!("no valid TAI bulletin: {}", errors.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulletin_chooses_current_leap_and_checks_expiry() {
        let text = "#@ 4200000000\n3692217600 37 # 2017\n4102444800 38 # future\n";
        assert_eq!(parse_ietf_bulletin(text, 1_700_000_000).unwrap(), 37);
        assert!(parse_ietf_bulletin(text, 2_000_000_000).is_err());
    }
}
