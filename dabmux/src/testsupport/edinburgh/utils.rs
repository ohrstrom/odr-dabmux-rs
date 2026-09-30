use super::tables::EBU_LATIN_TO_UNICODE;

pub fn decode_chars(chars: &[u8], charset: u8) -> String {
    match charset {
        0xF => String::from_utf8_lossy(chars).to_string(),
        0x4 => chars.iter().map(|&b| b as char).collect(),
        0x0 => chars
            .iter()
            .map(|&b| char::from_u32(EBU_LATIN_TO_UNICODE[b as usize] as u32).unwrap_or('?'))
            .collect(),
        _ => format!("[unsupported charset 0x{:X}]", charset),
    }
}

// CRC-16 CCITT
pub fn calc_crc16_ccitt(data: &[u8]) -> u16 {
    let initial_invert = true;
    let final_invert = true;
    let gen_polynom: u16 = 0x1021;

    let mut crc: u16 = if initial_invert { 0xFFFF } else { 0x0000 };

    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ gen_polynom;
            } else {
                crc <<= 1;
            }
        }
    }

    if final_invert {
        crc ^= 0xFFFF;
    }

    crc
}
