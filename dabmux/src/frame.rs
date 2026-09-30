//! Deterministic 24 ms frame timing and ETI(NI) assembly.

use anyhow::{bail, Result};

use crate::edi::crc16;

pub const FRAME_PERIOD_MS: u64 = 24;
pub const MAX_ETI_BYTES: usize = 6144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameClock {
    pub count: u64,
    pub unix_seconds: i64,
    pub millisecond: u16,
}

impl FrameClock {
    pub fn from_unix_millis(unix_millis: u64) -> Result<Self> {
        let aligned = unix_millis / FRAME_PERIOD_MS * FRAME_PERIOD_MS;
        Self::new(
            aligned / FRAME_PERIOD_MS,
            (aligned / 1000) as i64,
            (aligned % 1000) as u16,
        )
    }

    /// Align wall time to the C++ 24 ms PPS grid and choose the matching FCT phase.
    pub fn from_wall_time(unix_millis: u64, tist_at_fct0_ms: u16) -> Result<Self> {
        if tist_at_fct0_ms >= 1000 {
            bail!("TIST at FCT zero must be below one second");
        }
        let seconds = unix_millis / 1000;
        let rounded = ((unix_millis % 1000 + 12) / FRAME_PERIOD_MS) * FRAME_PERIOD_MS;
        let absolute = seconds * 1000 + rounded;
        let offset_count = rounded / FRAME_PERIOD_MS;
        let counter_offset = u64::from(tist_at_fct0_ms) / FRAME_PERIOD_MS;
        let count = (250 - counter_offset + offset_count) % 250;
        Self::new(count, (absolute / 1000) as i64, (absolute % 1000) as u16)
    }

    pub fn new(count: u64, unix_seconds: i64, millisecond: u16) -> Result<Self> {
        if millisecond >= 1000 {
            bail!("millisecond offset must be below 1000");
        }
        Ok(Self {
            count,
            unix_seconds,
            millisecond,
        })
    }

    pub fn tick(&mut self) {
        self.count += 1;
        self.millisecond += FRAME_PERIOD_MS as u16;
        if self.millisecond >= 1000 {
            self.millisecond -= 1000;
            self.unix_seconds += 1;
        }
    }

    pub fn shift_millis(&mut self, delta: i64) -> Result<()> {
        let millis = self
            .unix_seconds
            .checked_mul(1000)
            .and_then(|value| value.checked_add(i64::from(self.millisecond)))
            .and_then(|value| value.checked_add(delta))
            .ok_or_else(|| anyhow::anyhow!("clock offset overflow"))?;
        self.unix_seconds = millis.div_euclid(1000);
        self.millisecond = millis.rem_euclid(1000) as u16;
        Ok(())
    }

    pub fn rephase_fct0(&mut self, tist_at_fct0_ms: u16) -> Result<()> {
        if tist_at_fct0_ms >= 1000 {
            bail!("TIST at FCT zero must be below one second");
        }
        let current_count = (u64::from(self.millisecond) + 12) / FRAME_PERIOD_MS;
        let target_count = u64::from(tist_at_fct0_ms) / FRAME_PERIOD_MS;
        self.count = self.count / 250 * 250 + (250 - target_count + current_count) % 250;
        Ok(())
    }

    pub fn tsta(&self) -> u32 {
        (u32::from(self.millisecond) * 16_384) % 0xfa_0000
    }

    pub fn dlfc(&self) -> u16 {
        (self.count % 5000) as u16
    }
    pub fn fct(&self) -> u8 {
        (self.count % 250) as u8
    }
    pub fn fp(&self) -> u8 {
        (self.count & 7) as u8
    }
}

/// MNSC carries UTC date/time over four frame phases.
pub fn mnsc(unix_seconds: i64, frame_phase: u8) -> u16 {
    let day = unix_seconds.div_euclid(86_400);
    let sod = unix_seconds.rem_euclid(86_400);
    let second = (sod % 60) as u8;
    let minute = ((sod / 60) % 60) as u8;
    let hour = (sod / 3600) as u8;
    let (year, month, day_of_month) = civil_from_days(day);
    let bcd = |value: u8| (value / 10) << 4 | (value % 10);
    let [lo, hi] = match frame_phase & 3 {
        0 => [0, 0],
        1 => [bcd(second) | 0x80, bcd(minute) | 0x80],
        2 => [bcd(hour), bcd(day_of_month)],
        _ => [bcd(month), bcd(year.rem_euclid(100) as u8)],
    };
    u16::from_le_bytes([lo, hi])
}

fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u8, day as u8)
}

#[derive(Debug, Clone, Copy)]
pub struct Stream<'a> {
    pub id: u8,
    pub start_address_cu: u16,
    pub tpl: u8,
    pub payload: &'a [u8],
}

#[derive(Debug, Clone)]
pub struct EtiFrame {
    pub bytes: Vec<u8>,
    pub fic: Vec<u8>,
    pub mnsc: u16,
}

pub fn assemble(
    mode: u8,
    clock: FrameClock,
    mnsc: u16,
    fic: &[u8],
    streams: &[Stream<'_>],
    tist_enabled: bool,
) -> Result<EtiFrame> {
    let (mid, fic_len) = match mode {
        1 => (1u8, 96),
        2 => (2, 96),
        3 => (3, 128),
        4 => (0, 96),
        _ => bail!("invalid DAB mode"),
    };
    if fic.len() != fic_len || streams.len() > 64 {
        bail!("invalid FIC or stream count");
    }
    let mut payload_words = 0usize;
    for stream in streams {
        if stream.id >= 64
            || stream.start_address_cu > 1023
            || stream.tpl > 63
            || !stream.payload.len().is_multiple_of(8)
        {
            bail!("invalid stream characteristics");
        }
        payload_words = payload_words
            .checked_add(stream.payload.len() / 4)
            .ok_or_else(|| anyhow::anyhow!("payload overflow"))?;
    }
    let fl = 1 + fic_len / 4 + streams.len() + payload_words;
    let size = (fl + 4) * 4;
    if fl > 0x7ff || size > MAX_ETI_BYTES {
        bail!("ETI frame too large");
    }
    let mut bytes = vec![0u8; size];
    let sync = if clock.count.is_multiple_of(2) {
        0x49_c5_f8
    } else {
        0xb6_3a_07
    };
    bytes[0] = 0xff;
    bytes[1..4].copy_from_slice(&u32::to_le_bytes(sync)[..3]);
    bytes[4] = clock.fct();
    bytes[5] = 0x80 | streams.len() as u8;
    bytes[6] = ((clock.fp() & 7) << 5) | ((mid & 3) << 3) | ((fl >> 8) as u8 & 7);
    bytes[7] = fl as u8;
    let mut cursor = 8;
    for stream in streams {
        let stl = stream.payload.len() / 8;
        if stl > 1023 {
            bail!("stream exceeds ETI STL range");
        }
        bytes[cursor] = (stream.id << 2) | ((stream.start_address_cu >> 8) as u8 & 3);
        bytes[cursor + 1] = stream.start_address_cu as u8;
        bytes[cursor + 2] = (stream.tpl << 2) | ((stl >> 8) as u8 & 3);
        bytes[cursor + 3] = stl as u8;
        cursor += 4;
    }
    bytes[cursor..cursor + 2].copy_from_slice(&mnsc.to_le_bytes());
    let header_crc = crc16(&bytes[4..cursor + 2]);
    bytes[cursor + 2..cursor + 4].copy_from_slice(&header_crc.to_be_bytes());
    cursor += 4;
    bytes[cursor..cursor + fic_len].copy_from_slice(fic);
    cursor += fic_len;
    for stream in streams {
        bytes[cursor..cursor + stream.payload.len()].copy_from_slice(stream.payload);
        cursor += stream.payload.len();
    }
    let mst_start = 12 + 4 * streams.len();
    let mst_crc = crc16(&bytes[mst_start..cursor]);
    bytes[cursor..cursor + 2].copy_from_slice(&mst_crc.to_be_bytes());
    bytes[cursor + 2..cursor + 4].copy_from_slice(&0xffffu16.to_be_bytes());
    cursor += 4;
    let tist = if tist_enabled {
        clock.tsta()
    } else {
        0xff_ffff
    };
    bytes[cursor..cursor + 4].copy_from_slice(&((tist << 8) | 0xff).to_be_bytes());
    Ok(EtiFrame {
        bytes,
        fic: fic.to_vec(),
        mnsc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_crosses_second_and_counter_boundary() {
        let mut clock = FrameClock::new(249, 100, 992).unwrap();
        assert_eq!(clock.fct(), 249);
        clock.tick();
        assert_eq!(
            (
                clock.count,
                clock.unix_seconds,
                clock.millisecond,
                clock.fct()
            ),
            (250, 101, 16, 0)
        );
    }

    #[test]
    fn startup_clock_uses_one_aligned_instant() {
        let clock = FrameClock::from_unix_millis(1_704_164_645_999).unwrap();
        assert_eq!(
            clock.unix_seconds * 1000 + i64::from(clock.millisecond),
            (1_704_164_645_999u64 / 24 * 24) as i64
        );
        assert_eq!(clock.count, 1_704_164_645_999 / 24);
    }

    #[test]
    fn hot_tist_offset_moves_timestamp_without_resetting_frame_count() {
        let mut clock = FrameClock::new(250, 1_700_000_000, 992).unwrap();
        clock.shift_millis(2_000).unwrap();
        assert_eq!(clock.count, 250);
        assert_eq!(
            (clock.unix_seconds, clock.millisecond),
            (1_700_000_002, 992)
        );
        clock.shift_millis(-2_024).unwrap();
        assert_eq!(
            (clock.unix_seconds, clock.millisecond),
            (1_700_000_000, 968)
        );
    }

    #[test]
    fn fct_zero_phase_matches_requested_tist_phase() {
        let clock = FrameClock::from_wall_time(1_700_000_000_480, 0).unwrap();
        assert_eq!(clock.fct(), 20);
        assert_eq!(clock.millisecond, 480);
        let mut shifted = clock;
        shifted.rephase_fct0(240).unwrap();
        assert_eq!(shifted.fct(), 10);
        assert_eq!(shifted.millisecond, 480);
    }

    #[test]
    fn assembles_bounded_frame_and_crcs() {
        let clock = FrameClock::new(0, 0, 0).unwrap();
        let fic = [0xffu8; 96];
        let stream = Stream {
            id: 1,
            start_address_cu: 0,
            tpl: 0x22,
            payload: &[0x55; 288],
        };
        let frame = assemble(1, clock, 0, &fic, &[stream], false).unwrap();
        assert_eq!(frame.bytes.len(), (1 + 24 + 1 + 72 + 4) * 4);
        assert_eq!(frame.bytes[0..4], [0xff, 0xf8, 0xc5, 0x49]);
        assert_eq!(&frame.bytes[8..12], &[4, 0, 0x88, 0x24]);
        assert_eq!(&frame.bytes[12..14], &0u16.to_le_bytes());
        assert_eq!(
            &frame.bytes[14..16],
            &crc16(&frame.bytes[4..14]).to_be_bytes()
        );
        assert_eq!(&frame.bytes[frame.bytes.len() - 4..], &[0xff; 4]);
    }

    #[test]
    fn reference_header_fields_match_captured_cpp_frame() {
        // C++ v5.5.1-dirty, one 96 kb/s EEP-A level 3 service, FCT 38.
        let clock = FrameClock::new(38, 0, 0).unwrap();
        let fic = [0u8; 96];
        let stream = Stream {
            id: 1,
            start_address_cu: 0,
            tpl: 0x22,
            payload: &[0; 288],
        };
        let frame = assemble(1, clock, 0x3009, &fic, &[stream], false).unwrap();
        assert_eq!(
            &frame.bytes[..16],
            &[
                0xff, 0xf8, 0xc5, 0x49, 0x26, 0x81, 0xc8, 0x62, 0x04, 0, 0x88, 0x24, 0x09, 0x30,
                0x4e, 0x00
            ]
        );
    }

    #[test]
    fn mnsc_encodes_utc_calendar() {
        // 2024-01-02 03:04:05 UTC.
        let timestamp = 1_704_164_645;
        assert_eq!(mnsc(timestamp, 1), 0x8485);
        assert_eq!(mnsc(timestamp, 2), 0x0203);
        assert_eq!(mnsc(timestamp, 3), 0x2401);
    }
}
