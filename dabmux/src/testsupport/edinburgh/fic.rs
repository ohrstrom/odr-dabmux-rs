use super::utils;
use super::utils::decode_chars;
use serde::Serialize;
use thiserror::Error;

use super::tables;

#[derive(Debug, Serialize)]
pub struct Subchannel {
    pub id: u8,
    pub start: usize,
    pub size: Option<usize>,
    pub pl: Option<String>,
    pub bitrate: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct Fig0 {
    pub cn: bool,
    oe: bool,
    pd: bool,
    ext: u8,
}

#[derive(Debug, Serialize)]
pub struct Fig1 {
    charset: u8,
    oe: bool,
    ext: u8,
}

// FIG 0s
#[derive(Debug, Serialize)]
pub struct Fig0_0 {
    base: Fig0,
    pub eid: u16,
    pub al_flag: bool,
    /// Local addition: change flags, CIF count and occurrence change.
    pub change_flags: u8,
    pub cif_count: u16,
    pub occurrence_change: Option<u8>,
}
impl Fig0_0 {
    // FIG 0/0 - Ensemble information (MCI)
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        if data.len() < 4 {
            return Err(FigError::InvalidSize { l: data.len() });
        }

        // 16-bit Ensemble ID (Big-Endian)
        let eid = u16::from_be_bytes([data[0], data[1]]);

        // change flags (bits 7-6 of data[2])
        let change_flags = data[2] >> 6;

        // alarm flag (bit 5 of data[2])
        let al_flag = (data[2] & 0x20) != 0;

        // CIF count: modulo-20 high part, modulo-250 low part
        let cif_count = u16::from(data[2] & 0x1f) * 250 + u16::from(data[3]);

        // occurrence change, present unless the change flags are 00
        let occurrence_change = match (change_flags, data.get(4)) {
            (0, _) => None,
            (_, Some(occurrence)) => Some(*occurrence),
            (_, None) => return Err(FigError::InvalidSize { l: data.len() }),
        };

        Ok(Self {
            base,
            eid,
            al_flag,
            change_flags,
            cif_count,
            occurrence_change,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Fig0_1 {
    pub base: Fig0,
    pub subchannels: Vec<Subchannel>,
}

impl Fig0_1 {
    // FIG 0/1 - Sub-channel organization (MCI)
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        let mut offset = 0;
        let mut subchannels = Vec::new();

        while offset < data.len() {
            if offset + 2 > data.len() {
                return Err(FigError::InvalidSize { l: data.len() });
            }

            let id = data[offset] >> 2;
            let start = ((data[offset] & 0x03) as usize) << 8 | data[offset + 1] as usize;
            offset += 2;

            let mut size = None;
            let mut pl = None;
            let mut bitrate = None;

            let short_long_form = data.get(offset).map(|&b| b & 0x80 != 0).unwrap_or(false);

            if short_long_form {
                // long form
                if offset + 1 >= data.len() {
                    return Err(FigError::InvalidSize { l: data.len() });
                }

                let option = (data[offset] & 0x70) >> 4;
                let pl_index = (data[offset] & 0x0C) >> 2;
                let subch_size = ((data[offset] & 0x03) as usize) << 8 | data[offset + 1] as usize;
                offset += 2;

                match option {
                    0b000 => {
                        size = Some(subch_size);
                        pl = Some(format!("EEP {}-A", pl_index + 1));
                        bitrate = Some(subch_size / EEP_A_SIZE_FACTORS[pl_index as usize] * 8);
                    }
                    0b001 => {
                        size = Some(subch_size);
                        pl = Some(format!("EEP {}-B", pl_index + 1));
                        bitrate = Some(subch_size / EEP_B_SIZE_FACTORS[pl_index as usize] * 32);
                    }
                    _ => {}
                }
            } else {
                // short form
                let table_switch = data.get(offset).map(|&b| b & 0x40 != 0).unwrap_or(false);
                if !table_switch {
                    let table_index = (data[offset] & 0x3F) as usize;
                    if table_index < UEP_SIZES.len() {
                        size = Some(UEP_SIZES[table_index]);
                        pl = Some(format!("UEP {}", UEP_PLS[table_index]));
                        bitrate = Some(UEP_BITRATES[table_index]);
                    }
                }
                offset += 1;
            }

            // Ignore sc_id > 30
            if id <= 30 {
                subchannels.push(Subchannel {
                    id,
                    start,
                    size,
                    pl,
                    bitrate,
                });
            }

            // if id >= 30 {
            //     println!("SCID: {} - start: {} size: {:?} bitrate: {:?}", id, start, size, bitrate);
            // }
        }

        Ok(Self { base, subchannels })
    }
}

#[derive(Debug, Serialize)]
pub struct Fig0_2 {
    base: Fig0,
    pub services: Vec<ServiceComponent>,
}

#[derive(Debug, Serialize)]
pub struct ServiceComponent {
    pub sid: u16,
    pub tmid: u8,
    pub scid: u8,
    pub primary: bool,
    pub ca: bool,
}

impl Fig0_2 {
    // FIG 0/2 - Service organization (MCI)
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        let mut offset = 0;
        let mut services = Vec::new();

        while offset + 2 <= data.len() {
            // service ID (SID) - first two bytes
            let sid = u16::from_be_bytes([data[offset], data[offset + 1]]);
            offset += 2;

            // check remaining bytes
            if offset >= data.len() {
                return Err(FigError::InvalidSize { l: data.len() });
            }

            let num_components = data[offset] & 0x0F; // Number of service components
            offset += 1;

            for _ in 0..num_components {
                if offset + 1 >= data.len() {
                    return Err(FigError::InvalidSize { l: data.len() });
                }

                let tmid = (data[offset] & 0xC0) >> 6; // transport Mechanism ID
                let _ascty = data[offset] & 0x3F; // audio Service Type (ignored)
                let scid = data[offset + 1] >> 2; // subchannel ID
                let primary = (data[offset + 1] & 0x02) != 0; // primary component flag
                let ca = (data[offset + 1] & 0x01) != 0; // conditional Access flag
                offset += 2;

                // astci  0: DAB
                // ascti 63: DAB+
                // log::debug!("ASCTI: {}", ascty);

                // ignore CA (Conditional Access) components
                if !ca {
                    services.push(ServiceComponent {
                        sid,
                        tmid,
                        scid,
                        primary,
                        ca,
                    });

                    // log::debug!("FIG0/2: SID: 0x{:04X}, TMID: {}, scid: {}, Primary: {}, CA: {}",
                    //            sid, tmid, scid, primary, ca);
                }
            }
        }

        Ok(Self { base, services })
    }
}

#[derive(Debug, Serialize)]
pub struct Fig0_3 {
    base: Fig0,
    pub scid: u16,           // 12 bits
    pub rfa: u8,             // 3 bits
    pub scca_flag: bool,     // 1 bit
    pub dg_flag: bool,       // 1 bit
    pub rfu: bool,           // 1 bit
    pub dscty: u8,           // 6 bits
    pub subchid: u8,         // 6 bits
    pub packet_address: u16, // 10 bits
    pub scca: Option<u16>,   // present if scca_flag
}

impl Fig0_3 {
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        if data.len() < 5 {
            return Err(FigError::InvalidSize { l: data.len() });
        }

        let b0 = data[0];
        let b1 = data[1];
        let b2 = data[2];
        let b3 = data[3];
        let b4 = data[4];

        let scid = ((b0 as u16) << 4) | ((b1 as u16) >> 4);
        let rfa = (b1 & 0x0E) >> 1;
        let scca_flag = (b1 & 0x01) != 0;

        let dg_flag = (b2 & 0x80) != 0;
        let rfu = (b2 & 0x40) != 0;
        let dscty = b2 & 0x3F;

        let subchid = (b3 >> 2) & 0x3F;
        let packet_address = (((b3 & 0x03) as u16) << 8) | (b4 as u16);

        let scca = if scca_flag {
            if data.len() < 7 {
                return Err(FigError::InvalidSize { l: data.len() });
            }
            Some(u16::from_be_bytes([data[5], data[6]]))
        } else {
            None
        };

        Ok(Self {
            base,
            scid,
            rfa,
            scca_flag,
            dg_flag,
            rfu,
            dscty,
            subchid,
            packet_address,
            scca,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Fig0_5 {
    base: Fig0,
    pub services: Vec<ServiceLanguage>,
}

#[derive(Debug, Serialize)]
pub struct ServiceLanguage {
    pub scid: u8,
    pub language: tables::Language,
}

impl Fig0_5 {
    // FIG 0/5 - Service component language (SI)
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        if data.len() < 3 {
            return Err(FigError::InvalidSize { l: data.len() });
        }

        let mut services = Vec::new();
        let mut offset = 0;

        while offset + 1 < data.len() {
            let byte = data[offset];
            let ls_flag = (byte & 0x80) != 0;

            if ls_flag {
                // long form — skip 3 bytes
                // log::debug!("FIG0/5: Long form detected, skipping 3 bytes");
                offset += 3;
                continue;
            }
            let msc_fic_flag = (byte & 0x40) != 0;
            if !msc_fic_flag {
                let scid = byte & 0x3F;
                let language = data[offset + 1];

                services.push(ServiceLanguage {
                    scid,
                    language: tables::Language::from(language),
                });
            }

            offset += 2;
        }

        // log::debug!("FIG0/5: {:?} - SVC: {:?}", base, services);
        // log::debug!("FIG0/5: SVC: {:?}", services);

        Ok(Self { base, services })
    }
}

#[derive(Debug, Serialize)]
pub struct Fig0_9 {
    base: Fig0,
    pub lto: i32,
    pub ecc: u8,
    pub int_table_id: u8,
}

impl Fig0_9 {
    // FIG 0/9 - Country, LTO & International table (SI)
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        if data.len() < 3 {
            return Err(FigError::InvalidSize { l: data.len() });
        }

        // log::debug!("FIG0/9: {:?} - SVC: {:?} - {} bytes", base, data, data.len());

        let ext_flag = (data[0] & 0x80) != 0;
        let lto_raw = data[0] & 0x3F;
        let lto_sign = if (lto_raw & 0x20) == 0 { 1 } else { -1 };

        let lto_half_hours = lto_raw & 0x1F;

        let lto = lto_sign * (lto_half_hours as i32) / 2;
        let ecc = data[1];
        let int_table_id = data[2];

        // log::debug!("ECC: {} int_table_id: {}", ecc, int_table_id);

        /*
        Extended field: this n × 8-bit field shall contain one or more sub-fields, which define those services for which their
        ECC differs from that of the ensemble.
        */
        if ext_flag {
            let mut idx = 3;
            while idx + 3 <= data.len() {
                let byte = data[idx];
                let num_services = byte >> 6;
                let _ecc = data
                    .get(idx + 1)
                    .copied()
                    .ok_or(FigError::InvalidSize { l: data.len() })?;

                let mut sids = Vec::new();
                for i in 0..num_services {
                    let sid_offset = idx + 2 + (i as usize) * 2;
                    if sid_offset + 1 >= data.len() {
                        return Err(FigError::InvalidSize { l: data.len() });
                    }
                    let sid = u16::from_be_bytes([data[sid_offset], data[sid_offset + 1]]);
                    sids.push(sid);
                }

                let size = 2 + (num_services as usize) * 2;
                idx += size;

                // log::debug!("FIG0/9: Extended subfield: ecc: {} sids: {:?}", ecc, sids);

                // TODO: we somehow want to store this ;)
            }
        }

        // log::debug!("FIG0/9 ext: {} lto: {} ecc: {} int_table_id: {}", ext_flag, lto, ecc, int_table_id);

        Ok(Self {
            base,
            lto,
            ecc,
            int_table_id,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Fig0_10 {
    base: Fig0,
    pub mjd: u32,
    pub lsi: bool,
    pub utc_flag: bool,
    pub utc: DateTimeUTC,
}

#[derive(Debug, Serialize)]
pub enum DateTimeUTC {
    Short {
        year: i32,
        month: u8,
        day: u8,
        hours: u8,
        minutes: u8,
    },
    Long {
        year: i32,
        month: u8,
        day: u8,
        hours: u8,
        minutes: u8,
        seconds: u8,
        milliseconds: u16,
    },
}

impl Fig0_10 {
    // FIG 0/10 - Date & time (SI)
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        if data.len() < 4 {
            return Err(FigError::InvalidSize { l: data.len() });
        }

        // log::debug!("FIG0/10: {:?} - SVC: {:?}", base, data);

        // correct MJD extraction: 17 bits from data[0], data[1], and top 2 bits of data[2]
        let mjd =
            (((data[0] & 0x7F) as u32) << 10) | ((data[1] as u32) << 2) | ((data[2] as u32) >> 6);

        // inline MJD > Gregorian date conversion. consider moving this to a another place..
        let mjd_f = mjd as f64;
        let y0 = ((mjd_f - 15078.2) / 365.25).floor();
        let m0 = ((mjd_f - 14956.1 - (y0 * 365.25).floor()) / 30.6001).floor();
        let d = (mjd_f - 14956.0 - (y0 * 365.25).floor() - (m0 * 30.6001).floor()) as u8;
        let k = if m0 == 14.0 || m0 == 15.0 { 1.0 } else { 0.0 };
        let year = (y0 + k) as i32 + 1900;
        let month = (m0 - 1.0 - k * 12.0) as u8;
        let day = d;

        let lsi = ((data[2] >> 5) & 0x01) != 0;
        let utc_flag = ((data[2] >> 3) & 0x01) != 0;

        let utc = if utc_flag {
            if data.len() < 6 {
                log::warn!(
                    "FIG0/10: Invalid size for long form UTC: {} bytes",
                    data.len()
                );
                return Err(FigError::InvalidSize { l: data.len() });
            }

            let hour = ((data[2] & 0x07) << 2) | (data[3] >> 6);
            let minute = data[3] & 0x3F;
            let second = data[4] >> 2;
            let millisecond = ((data[4] & 0x03) as u16) << 8 | data[5] as u16;

            DateTimeUTC::Long {
                year,
                month,
                day,
                hours: hour,
                minutes: minute,
                seconds: second,
                milliseconds: millisecond,
            }
        } else {
            if data.len() < 6 {
                log::warn!(
                    "FIG0/10: Invalid size for short form UTC: {} bytes",
                    data.len()
                );
                return Err(FigError::InvalidSize { l: data.len() });
            }

            let b4 = data[4];
            let b5 = data[5];

            let hour = (b4 >> 3) & 0x1F;
            let minute = ((b4 & 0x07) << 3) | (b5 >> 5);

            DateTimeUTC::Short {
                year,
                month,
                day,
                hours: hour,
                minutes: minute,
            }
        };

        Ok(Self {
            base,
            mjd,
            lsi,
            utc_flag,
            utc,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct Fig0_13 {
    base: Fig0,
    pub services: Vec<ServiceUA>,
}

#[derive(Debug, Serialize)]
pub struct ServiceUA {
    pub sid: u16,
    pub scids: u8,
    pub uas: Vec<tables::UserApplication>,
}

impl Fig0_13 {
    pub fn from_bytes(base: Fig0, data: &[u8]) -> Result<Self, FigError> {
        let mut services = Vec::new();
        let mut offset = 0;

        while offset + 3 <= data.len() {
            let sid = u16::from_be_bytes([data[offset], data[offset + 1]]);
            offset += 2;

            let scids = data[offset] >> 4;
            let num_uas = data[offset] & 0x0F;
            offset += 1;

            if num_uas == 0 {
                break;
            }

            if num_uas > 6 {
                log::warn!("FIG0/13: Invalid number of User Applications: {num_uas}");
                break;
            }

            let mut uas = Vec::new();

            for _ in 0..num_uas {
                if offset + 2 > data.len() {
                    log::warn!("FIG0/13: Unexpected end of buffer before UA entry");
                    break;
                }

                let ua_type = ((data[offset] as u16) << 3) | ((data[offset + 1] >> 5) as u16);
                let ua_data_length = data[offset + 1] & 0x1F;
                offset += 2;

                if offset + ua_data_length as usize > data.len() {
                    log::warn!(
                        "FIG0/13: UA data ({} bytes) exceeds buffer (remaining: {})",
                        ua_data_length,
                        data.len() - offset
                    );
                    break;
                }

                let _ua_data = &data[offset..offset + ua_data_length as usize];
                offset += ua_data_length as usize;

                uas.push(tables::UserApplication::from(ua_type));
            }

            services.push(ServiceUA { sid, scids, uas });
        }

        Ok(Self { base, services })
    }
}

// FIG 1s
#[derive(Debug, Serialize)]
pub struct Fig1_0 {
    base: Fig1,
    pub eid: u16,
    pub label: String,
    pub short_label: String,
}
impl Fig1_0 {
    pub fn from_bytes(base: Fig1, data: &[u8]) -> Result<Self, FigError> {
        if data.len() < 18 {
            return Err(FigError::InvalidSize { l: data.len() });
        }

        let eid = u16::from_be_bytes([data[0], data[1]]);
        let label = decode_chars(&data[2..18], base.charset)
            .trim_end()
            .to_string();
        let short_label =
            Self::derive_short_label(&label, u16::from_be_bytes([data[18], data[19]]));

        Ok(Self {
            base,
            eid,
            label,
            short_label,
        })
    }

    fn derive_short_label(label: &str, mask: u16) -> String {
        let mut short = String::new();

        for (i, ch) in label.chars().enumerate() {
            if mask & (0x8000 >> i) != 0 {
                short.push(ch);
            }
        }

        short
    }
}

#[derive(Debug, Serialize)]
pub struct Fig1_1 {
    base: Fig1,
    pub sid: u16,
    pub label: String,
    pub short_label: String,
}

impl Fig1_1 {
    pub fn from_bytes(base: Fig1, data: &[u8]) -> Result<Self, FigError> {
        if data.len() < 18 {
            return Err(FigError::InvalidSize { l: data.len() });
        }

        let sid = u16::from_be_bytes([data[0], data[1]]);
        let label = decode_chars(&data[2..18], base.charset)
            .trim_end()
            .to_string();
        let short_label =
            Self::derive_short_label(&label, u16::from_be_bytes([data[18], data[19]]));

        Ok(Self {
            base,
            sid,
            label,
            short_label,
        })
    }

    fn derive_short_label(label: &str, mask: u16) -> String {
        let mut short = String::new();

        for (i, ch) in label.chars().enumerate() {
            if mask & (0x8000 >> i) != 0 {
                short.push(ch);
            }
        }

        short
    }
}

#[derive(Debug, Serialize)]
pub struct Fig1_4 {
    base: Fig1,
}
impl Fig1_4 {
    pub fn from_bytes(base: Fig1, _data: &[u8]) -> Result<Self, FigError> {
        // implement decoding here

        Ok(Self { base })
    }
}

#[derive(Debug, Serialize)]
pub enum Fig {
    F0_0(Fig0_0),
    F0_1(Fig0_1),
    F0_2(Fig0_2),
    F0_3(Fig0_3),
    F0_5(Fig0_5),
    F0_9(Fig0_9),
    F0_10(Fig0_10),
    F0_13(Fig0_13),
    //
    F1_0(Fig1_0),
    F1_1(Fig1_1),
    F1_4(Fig1_4),
}

#[derive(Debug, Error)]
pub enum FigError {
    #[error("Unsupported FIG: {kind}")]
    Unsupported { kind: u8 },

    #[error("Missing FIG data")]
    InvalidSize { l: usize },

    #[error("Missing FIG data")]
    NoData,
}

#[derive(Debug, Error)]
pub enum FicError {
    #[error("Invalid FIC size: {l}")]
    SizeInvalid { l: usize },

    #[error("FIG error: {0}")]
    FigError(#[from] FigError), // converts FigError to FicError
}

#[derive(Debug)]
pub struct FicDecoder {
    #[allow(dead_code)]
    eid: Option<String>,
}

impl FicDecoder {
    pub fn from_bytes(data: &[u8]) -> Result<Vec<Fig>, FicError> {
        if (data.len() % 32) != 0 {
            return Err(FicError::SizeInvalid { l: data.len() });
        }

        let mut figs: Vec<Fig> = Vec::new();

        for chunk in data.chunks(32) {
            figs.extend(Self::decode_fib(chunk)?);
        }

        Ok(figs)
    }

    fn decode_fib(data: &[u8]) -> Result<Vec<Fig>, FicError> {
        let crc_stored = u16::from_be_bytes([data[30], data[31]]);
        let crc_calculated = utils::calc_crc16_ccitt(&data[..30]);

        if crc_stored != crc_calculated {
            log::warn!("FicDecoder: Discarding FIB due to CRC mismatch");
        }

        let mut figs: Vec<Fig> = Vec::new();

        let mut offset = 0;

        while offset < 30 && data[offset] != 0xFF {
            let fig_type = data[offset] >> 5;
            let fig_length = (data[offset] & 0x1F) as usize;

            offset += 1;

            // primary type: 0 / 1
            match fig_type {
                0 => {
                    match Self::decode_fig0(&data[offset..offset + fig_length]) {
                        Ok(fig) => figs.push(fig),
                        Err(_e) => {}
                    };
                }
                1 => {
                    match Self::decode_fig1(&data[offset..offset + fig_length]) {
                        Ok(fig) => figs.push(fig),
                        Err(_e) => {}
                    };
                }
                _ => {
                    log::warn!("Unknown FIG type: {}", fig_type);
                }
            }

            offset += fig_length;
        }

        // log::debug!("FicDecoder: {} figs", figs.len());

        Ok(figs)
    }

    fn decode_fig0(data: &[u8]) -> Result<Fig, FigError> {
        if data.is_empty() {
            return Err(FigError::NoData);
        }

        let header = data[0];

        let cn = header & 0x80 != 0; // Bit 7
        let oe = header & 0x40 != 0; // Bit 6
        let pd = header & 0x20 != 0; // Bit 5
        let ext = header & 0x1F; // Bits 0-4

        // log::debug!("FIG0: cn: {}, oe: {}, pd: {}, ext: {}", cn, oe, pd, ext);

        let base = Fig0 { cn, oe, pd, ext };

        match ext {
            0 => Ok(Fig::F0_0(Fig0_0::from_bytes(base, &data[1..])?)),
            1 => Ok(Fig::F0_1(Fig0_1::from_bytes(base, &data[1..])?)),
            2 => Ok(Fig::F0_2(Fig0_2::from_bytes(base, &data[1..])?)),
            3 => Ok(Fig::F0_3(Fig0_3::from_bytes(base, &data[1..])?)),
            5 => Ok(Fig::F0_5(Fig0_5::from_bytes(base, &data[1..])?)),
            9 => Ok(Fig::F0_9(Fig0_9::from_bytes(base, &data[1..])?)),
            10 => Ok(Fig::F0_10(Fig0_10::from_bytes(base, &data[1..])?)),
            13 => Ok(Fig::F0_13(Fig0_13::from_bytes(base, &data[1..])?)),
            _ => Err(FigError::Unsupported { kind: ext }),
        }
    }

    fn decode_fig1(data: &[u8]) -> Result<Fig, FigError> {
        if data.is_empty() {
            return Err(FigError::NoData);
        }

        let header = data[0];

        let charset = header >> 4; // Upper 4 bits
        let oe = (header & 0x08) != 0; // Bit 3 (boolean)
        let ext = header & 0x07; // Lower 3 bits

        // log::debug!("FIG1: charset: {}, oe: {}, ext: {}", charset, oe, ext);

        let base = Fig1 { charset, oe, ext };

        match ext {
            0 => Ok(Fig::F1_0(Fig1_0::from_bytes(base, &data[1..])?)),
            1 => Ok(Fig::F1_1(Fig1_1::from_bytes(base, &data[1..])?)),
            4 => Ok(Fig::F1_4(Fig1_4::from_bytes(base, &data[1..])?)),
            _ => Err(FigError::Unsupported { kind: ext }),
        }
    }
}

const UEP_SIZES: [usize; 64] = [
    16, 21, 24, 29, 35, 24, 29, 35, 42, 52, 29, 35, 42, 52, 32, 42, 48, 58, 70, 40, 52, 58, 70, 84,
    48, 58, 70, 84, 104, 58, 70, 84, 104, 64, 84, 96, 116, 140, 80, 104, 116, 140, 168, 96, 116,
    140, 168, 208, 116, 140, 168, 208, 232, 128, 168, 192, 232, 280, 160, 208, 280, 192, 280, 416,
];

const UEP_PLS: [u8; 64] = [
    5, 4, 3, 2, 1, 5, 4, 3, 2, 1, 5, 4, 3, 2, 5, 4, 3, 2, 1, 5, 4, 3, 2, 1, 5, 4, 3, 2, 1, 5, 4, 3,
    2, 5, 4, 3, 2, 1, 5, 4, 3, 2, 1, 5, 4, 3, 2, 1, 5, 4, 3, 2, 1, 5, 4, 3, 2, 1, 5, 4, 2, 5, 3, 1,
];

const UEP_BITRATES: [usize; 64] = [
    32, 32, 32, 32, 32, 48, 48, 48, 48, 48, 56, 56, 56, 56, 64, 64, 64, 64, 64, 80, 80, 80, 80, 80,
    96, 96, 96, 96, 96, 112, 112, 112, 112, 128, 128, 128, 128, 128, 160, 160, 160, 160, 160, 192,
    192, 192, 192, 192, 224, 224, 224, 224, 224, 256, 256, 256, 256, 256, 320, 320, 320, 384, 384,
    384,
];

const EEP_A_SIZE_FACTORS: [usize; 4] = [12, 8, 6, 4];
const EEP_B_SIZE_FACTORS: [usize; 4] = [27, 21, 18, 15];
