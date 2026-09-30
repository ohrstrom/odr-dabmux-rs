//! EDI AF/TAG framing shared by input reception and output generation.

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

const MAX_AF_PAYLOAD: usize = 64 * 1024;

pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub name: [u8; 4],
    pub value: Vec<u8>,
}

impl Tag {
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        let bits = self
            .value
            .len()
            .checked_mul(8)
            .context("TAG length overflow")?;
        out.extend_from_slice(&self.name);
        out.extend_from_slice(&u32::try_from(bits)?.to_be_bytes());
        out.extend_from_slice(&self.value);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AfPacket {
    pub sequence: u16,
    pub tags: Vec<Tag>,
}

impl AfPacket {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.encode_with_alignment(8)
    }

    pub fn encode_with_alignment(&self, alignment: u8) -> Result<Vec<u8>> {
        if !matches!(alignment, 8 | 16) {
            bail!("TAG packet alignment must be 8 or 16");
        }
        let mut payload = Vec::new();
        for tag in &self.tags {
            tag.encode(&mut payload)?;
        }
        if alignment == 16 {
            Tag {
                name: *b"*dmy",
                value: vec![0; 8],
            }
            .encode(&mut payload)?;
        }
        payload.resize(payload.len().div_ceil(8) * 8, 0);
        if payload.len() > MAX_AF_PAYLOAD {
            bail!("AF payload too large");
        }
        let mut out = Vec::with_capacity(12 + payload.len());
        out.extend_from_slice(b"AF");
        out.extend_from_slice(&u32::try_from(payload.len())?.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        out.extend_from_slice(&[0x90, b'T']);
        out.extend_from_slice(&payload);
        let crc = crc16(&out);
        out.extend_from_slice(&crc.to_be_bytes());
        Ok(out)
    }

    pub fn decode(data: &[u8]) -> Result<Self> {
        if data.len() < 12 || &data[..2] != b"AF" {
            bail!("invalid AF header");
        }
        let len = u32::from_be_bytes(data[2..6].try_into()?) as usize;
        if len > MAX_AF_PAYLOAD || data.len() != 12 + len {
            bail!("invalid AF length: {}", len);
        }
        if data[8] != 0x90 || data[9] != b'T' {
            bail!("unsupported AF revision or payload type");
        }
        let expected = u16::from_be_bytes(data[10 + len..12 + len].try_into()?);
        if expected != crc16(&data[..10 + len]) {
            bail!("AF CRC mismatch");
        }
        let mut tags = Vec::new();
        let mut cursor = 10;
        while cursor + 8 <= 10 + len {
            if data[cursor..10 + len].iter().all(|byte| *byte == 0) {
                break;
            }
            let name = data[cursor..cursor + 4].try_into()?;
            let bits = u32::from_be_bytes(data[cursor + 4..cursor + 8].try_into()?) as usize;
            if !bits.is_multiple_of(8) {
                bail!("TAG length is not byte aligned");
            }
            let end = cursor
                .checked_add(8 + bits / 8)
                .context("TAG length overflow")?;
            if end > 10 + len {
                bail!("TAG extends past AF payload");
            }
            tags.push(Tag {
                name,
                value: data[cursor + 8..end].to_vec(),
            });
            cursor = end;
        }
        Ok(Self {
            sequence: u16::from_be_bytes(data[6..8].try_into()?),
            tags,
        })
    }
}

pub fn pointer_tag(protocol: [u8; 4]) -> Tag {
    let mut value = protocol.to_vec();
    value.extend_from_slice(&[0, 0, 0, 0]);
    Tag {
        name: *b"*ptr",
        value,
    }
}

/// EDI management fields common to one 24 ms multiplex frame.
pub struct Deti<'a> {
    pub dlfc: u16,
    pub stat: u8,
    pub mid: u8,
    pub fp: u8,
    pub mnsc: u16,
    pub timestamp: Option<(u8, u32, u32)>,
    pub fic: &'a [u8],
}

impl Deti<'_> {
    pub fn tag(&self) -> Result<Tag> {
        if self.dlfc >= 5000 || self.mid > 3 || self.fp > 7 || !matches!(self.fic.len(), 96 | 128) {
            bail!("invalid DETI fields");
        }
        let fct = self.dlfc % 250;
        let fcth = self.dlfc / 250;
        let mut header = fct | (fcth << 8) | 0x4000;
        if self.timestamp.is_some() {
            header |= 0x8000;
        }
        let eti_header = u32::from(self.mnsc)
            | (u32::from(self.fp) << 19)
            | (u32::from(self.mid) << 22)
            | (u32::from(self.stat) << 24);
        let mut value = Vec::with_capacity(14 + self.fic.len());
        value.extend_from_slice(&header.to_be_bytes());
        value.extend_from_slice(&eti_header.to_be_bytes());
        if let Some((utco, seconds, tsta)) = self.timestamp {
            if tsta > 0xff_ffff {
                bail!("TSTA exceeds 24 bits");
            }
            value.push(utco);
            value.extend_from_slice(&seconds.to_be_bytes());
            value.extend_from_slice(&tsta.to_be_bytes()[1..]);
        }
        value.extend_from_slice(self.fic);
        Ok(Tag {
            name: *b"deti",
            value,
        })
    }
}

pub struct Est<'a> {
    pub index: u8,
    pub scid: u8,
    pub start_address: u16,
    pub tpl: u8,
    pub payload: &'a [u8],
}

impl Est<'_> {
    pub fn tag(&self) -> Result<Tag> {
        if self.index == 0
            || self.scid >= 64
            || self.start_address > 1023
            || self.tpl > 63
            || !self.payload.len().is_multiple_of(8)
        {
            bail!("invalid EST fields");
        }
        let sstc = (u32::from(self.scid) << 18)
            | (u32::from(self.start_address) << 8)
            | (u32::from(self.tpl) << 2);
        let mut value = Vec::with_capacity(self.payload.len() + 3);
        value.extend_from_slice(&sstc.to_be_bytes()[1..]);
        value.extend_from_slice(self.payload);
        Ok(Tag {
            name: [b'e', b's', b't', self.index],
            value,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedPayload {
    pub dlfc: u16,
    pub utco: Option<u8>,
    pub seconds: Option<u32>,
    pub tsta: Option<u32>,
    pub stream_index: u16,
    pub bytes: Vec<u8>,
}

pub fn decode_sti_payload(packet: &AfPacket, stream_index: u16) -> Result<TimedPayload> {
    if !packet
        .tags
        .iter()
        .any(|tag| tag.name == *b"*ptr" && tag.value.starts_with(b"DSTI"))
    {
        bail!("AF packet is not DSTI");
    }
    let dsti = packet
        .tags
        .iter()
        .find(|tag| tag.name == *b"dsti")
        .context("missing dsti tag")?;
    if dsti.value.len() < 2 {
        bail!("short dsti tag");
    }
    let header = u16::from_be_bytes(dsti.value[..2].try_into()?);
    let dlfc = ((header >> 8) & 0x1f) * 250 + (header & 0xff);
    let mut offset = 2;
    if header & 0x8000 != 0 {
        offset += 3;
    }
    let (utco, seconds, tsta) = if header & 0x4000 != 0 {
        if dsti.value.len() < offset + 8 {
            bail!("short dsti timestamp");
        }
        let time = (
            Some(dsti.value[offset]),
            Some(u32::from_be_bytes(
                dsti.value[offset + 1..offset + 5].try_into()?,
            )),
            Some(u32::from_be_bytes([
                0,
                dsti.value[offset + 5],
                dsti.value[offset + 6],
                dsti.value[offset + 7],
            ])),
        );
        offset += 8;
        time
    } else {
        (None, None, None)
    };
    if header & 0x2000 != 0 {
        offset += 9;
    }
    if dsti.value.len() != offset {
        bail!("invalid dsti length");
    }
    let name = [b's', b's', (stream_index >> 8) as u8, stream_index as u8];
    let tag = packet
        .tags
        .iter()
        .find(|tag| tag.name == name)
        .context("missing requested STI stream")?;
    if tag.value.len() < 3 {
        bail!("short STI stream tag");
    }
    Ok(TimedPayload {
        dlfc,
        utco,
        seconds,
        tsta,
        stream_index,
        bytes: tag.value[3..].to_vec(),
    })
}

/// Decode the first STI-D payload in an RTP datagram (ETSI EN 300 797).
pub fn decode_sti_rtp(data: &[u8]) -> Result<TimedPayload> {
    if data.len() < 28 || data[0] >> 6 != 2 || data[1] & 0x7f != 34 {
        bail!("invalid STI RTP header");
    }
    let csrc_count = usize::from(data[0] & 0x0f);
    let mut at = 12 + 4 * csrc_count;
    if data.len() < at + 16 {
        bail!("short STI RTP packet");
    }
    if data[0] & 0x10 != 0 {
        bail!("RTP header extensions are not supported");
    }
    at += 1; // STAT
    if !matches!(&data[at..at + 3], [0x1f, 0x90, 0xca] | [0xe0, 0x6f, 0x35]) {
        bail!("invalid STI frame sync");
    }
    at += 3;
    let dfs = usize::from(u16::from_be_bytes(data[at..at + 2].try_into()?));
    if dfs == 0 {
        bail!("empty STI data field");
    }
    at += 2;
    let cfs = u16::from_be_bytes(data[at..at + 2].try_into()?);
    if cfs != 0 {
        bail!("STI control field is unsupported");
    }
    at += 2 + 2 + 2 + 1; // CFS, SPID, RFU/DL, RFU
    let dfctl = u16::from(data[at]);
    at += 1;
    let dfcth = u16::from(data[at] >> 3);
    let nst = usize::from(u16::from_be_bytes(data[at..at + 2].try_into()?) & 0x7ff);
    at += 2;
    if nst == 0 || data.len() < at + 4 * nst + 4 {
        bail!("invalid STI stream count");
    }
    let stl = usize::from(u16::from_be_bytes(data[at..at + 2].try_into()?) & 0x1fff);
    let crcstf = data[at + 3] & 0x80 != 0;
    at += 4 * nst + 4;
    let payload_len = stl
        .checked_sub(if crcstf { 2 } else { 0 })
        .context("invalid STI stream length")?;
    if payload_len > dfs || data.len() < at + payload_len {
        bail!("truncated STI payload");
    }
    Ok(TimedPayload {
        dlfc: dfcth * 250 + dfctl,
        utco: None,
        seconds: None,
        tsta: None,
        stream_index: 1,
        bytes: data[at..at + payload_len].to_vec(),
    })
}

/// Fragment an AF packet into non-FEC PFT datagrams below the usual IP MTU.
pub fn fragment_af(af: &[u8], sequence: u16) -> Result<Vec<Vec<u8>>> {
    AfPacket::decode(af)?;
    let count = af.len().div_ceil(1400);
    if count == 0 || count > 0xff_ffff {
        bail!("invalid PFT fragment count");
    }
    let fragment_size = af.len().div_ceil(count);
    let mut packets = Vec::with_capacity(count);
    for (index, chunk) in af.chunks(fragment_size).enumerate() {
        let mut packet = Vec::with_capacity(14 + chunk.len());
        packet.extend_from_slice(b"PF");
        packet.extend_from_slice(&sequence.to_be_bytes());
        packet.extend_from_slice(&(index as u32).to_be_bytes()[1..]);
        packet.extend_from_slice(&(count as u32).to_be_bytes()[1..]);
        packet.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        let crc = crc16(&packet);
        packet.extend_from_slice(&crc.to_be_bytes());
        packet.extend_from_slice(chunk);
        packets.push(packet);
    }
    Ok(packets)
}

#[derive(Default)]
pub struct PftReassembler {
    pending: BTreeMap<u16, Vec<Option<Vec<u8>>>>,
}

impl PftReassembler {
    pub fn push(&mut self, data: &[u8]) -> Result<Option<AfPacket>> {
        if data.len() < 14 || &data[..2] != b"PF" {
            bail!("invalid PFT header");
        }
        let sequence = u16::from_be_bytes(data[2..4].try_into()?);
        let index = u32::from_be_bytes([0, data[4], data[5], data[6]]) as usize;
        let count = u32::from_be_bytes([0, data[7], data[8], data[9]]) as usize;
        let flags_len = u16::from_be_bytes(data[10..12].try_into()?);
        if flags_len & 0xc000 != 0 {
            bail!("FEC/address PFT fragment is not supported");
        }
        let len = usize::from(flags_len);
        if count == 0 || count > 64 || index >= count || len > 1400 || data.len() != 14 + len {
            bail!("invalid PFT fragment length or index");
        }
        if crc16(&data[..12]) != u16::from_be_bytes(data[12..14].try_into()?) {
            bail!("PFT header CRC mismatch");
        }
        if self.pending.len() >= 8 && !self.pending.contains_key(&sequence) {
            self.pending.pop_first();
        }
        let parts = self
            .pending
            .entry(sequence)
            .or_insert_with(|| vec![None; count]);
        if parts.len() != count {
            bail!("inconsistent PFT fragment count");
        }
        if parts[index].is_none() {
            parts[index] = Some(data[14..].to_vec());
        }
        if parts.iter().all(Option::is_some) {
            let mut af = Vec::new();
            for part in self.pending.remove(&sequence).expect("present sequence") {
                af.extend_from_slice(&part.expect("complete fragment set"));
            }
            Ok(Some(AfPacket::decode(&af)?))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn af_round_trip_and_crc_rejection() {
        let packet = AfPacket {
            sequence: 42,
            tags: vec![
                pointer_tag(*b"DSTI"),
                Tag {
                    name: *b"dsti",
                    value: vec![0, 1],
                },
                Tag {
                    name: [b's', b's', 0, 1],
                    value: vec![0, 0, 0, 1, 2, 3],
                },
            ],
        };
        let mut bytes = packet.encode().unwrap();
        assert_eq!(AfPacket::decode(&bytes).unwrap(), packet);
        assert_eq!(decode_sti_payload(&packet, 1).unwrap().bytes, vec![1, 2, 3]);
        bytes[15] ^= 1;
        assert!(AfPacket::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("CRC"));
    }

    #[test]
    fn af_accepts_unpadded_audio_encoder_tag_payload() {
        let packet = AfPacket {
            sequence: 7,
            tags: vec![
                pointer_tag(*b"DSTI"),
                Tag {
                    name: *b"dsti",
                    value: vec![0, 1],
                },
                Tag {
                    name: [b's', b's', 0, 1],
                    value: vec![0x5a; 231],
                },
            ],
        };
        let mut payload = Vec::new();
        for tag in &packet.tags {
            tag.encode(&mut payload).unwrap();
        }
        assert_eq!(payload.len(), 265);
        let mut bytes = b"AF".to_vec();
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&packet.sequence.to_be_bytes());
        bytes.extend_from_slice(&[0x90, b'T']);
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&crc16(&bytes).to_be_bytes());
        assert_eq!(AfPacket::decode(&bytes).unwrap(), packet);
    }

    #[test]
    fn sti_stream_tag_contains_three_header_bytes_and_full_audio_frame() {
        let audio = vec![0x5a; 216];
        let mut stream = vec![0, 0, 0];
        stream.extend_from_slice(&audio);
        let packet = AfPacket {
            sequence: 1,
            tags: vec![
                pointer_tag(*b"DSTI"),
                Tag {
                    name: *b"dsti",
                    value: vec![0, 1],
                },
                Tag {
                    name: [b's', b's', 0, 1],
                    value: stream,
                },
            ],
        };
        assert_eq!(decode_sti_payload(&packet, 1).unwrap().bytes, audio);
    }

    #[test]
    fn sixteen_byte_alignment_appends_dmy_tag() {
        let packet = AfPacket {
            sequence: 1,
            tags: vec![pointer_tag(*b"DETI")],
        };
        let encoded = packet.encode_with_alignment(16).unwrap();
        let decoded = AfPacket::decode(&encoded).unwrap();
        assert_eq!(decoded.tags.last().unwrap().name, *b"*dmy");
        assert_eq!(decoded.tags.last().unwrap().value.len(), 8);
    }

    #[test]
    fn deti_and_est_fields() {
        let fic = [0xffu8; 96];
        let deti = Deti {
            dlfc: 251,
            stat: 0xff,
            mid: 1,
            fp: 3,
            mnsc: 0x1234,
            timestamp: Some((5, 99, 0x12_3456)),
            fic: &fic,
        }
        .tag()
        .unwrap();
        assert_eq!(&deti.value[..2], &[0xc1, 1]);
        assert_eq!(&deti.value[6..14], &[5, 0, 0, 0, 99, 0x12, 0x34, 0x56]);
        let payload = [7u8; 24];
        let est = Est {
            index: 1,
            scid: 2,
            start_address: 72,
            tpl: 0x22,
            payload: &payload,
        }
        .tag()
        .unwrap();
        assert_eq!(est.name, [b'e', b's', b't', 1]);
        assert_eq!(est.value.len(), 27);
        let af = AfPacket {
            sequence: 0,
            tags: vec![pointer_tag(*b"DETI"), deti, est],
        };
        assert_eq!(AfPacket::decode(&af.encode().unwrap()).unwrap(), af);
    }

    #[test]
    fn pft_fragments_reassemble_out_of_order() {
        let packet = AfPacket {
            sequence: 7,
            tags: vec![
                pointer_tag(*b"DSTI"),
                Tag {
                    name: *b"dsti",
                    value: vec![0; 2],
                },
                Tag {
                    name: [b's', b's', 0, 1],
                    value: vec![0x5a; 4096],
                },
            ],
        };
        let af = packet.encode().unwrap();
        let mut fragments = fragment_af(&af, 7).unwrap();
        assert!(fragments.len() > 1);
        fragments.reverse();
        let mut reassembler = PftReassembler::default();
        let mut received = None;
        for fragment in fragments {
            received = reassembler.push(&fragment).unwrap().or(received);
        }
        assert_eq!(received.unwrap(), packet);
    }

    #[test]
    fn sti_rtp_extracts_payload_and_rejects_truncation() {
        let mut packet = vec![0u8; 12];
        packet[0] = 0x80;
        packet[1] = 34;
        packet.extend_from_slice(&[0xff, 0x1f, 0x90, 0xca]);
        packet.extend_from_slice(&288u16.to_be_bytes());
        packet.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        packet.extend_from_slice(&[0x01, 0x20, 0, 0]);
        packet.extend_from_slice(&[0; 4]);
        packet.extend_from_slice(&[0x5a; 288]);
        assert_eq!(decode_sti_rtp(&packet).unwrap().bytes, vec![0x5a; 288]);
        packet.truncate(packet.len() - 1);
        assert!(decode_sti_rtp(&packet).is_err());
    }
}
