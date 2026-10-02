//! MSC packet mode (EN 300 401 clause 5.3.2) with the Reed-Solomon FEC that
//! clause 5.3.5 requires for every packet mode sub-channel.
//!
//! Data packets fill a 12 × 188 byte Application Data Table column by column.
//! When it is full, each row gets 16 RS(204,188) parity bytes, which follow
//! the data packets in nine 24-byte FEC packets at address 1022.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::{Arc, OnceLock};

use crate::edi::crc16;

/// Smallest packet; logical frames and the data table are multiples of it.
pub const PACKET_UNIT: usize = 24;
pub const PADDING_ADDRESS: u16 = 0;
pub const FEC_ADDRESS: u16 = 1022;

const ROWS: usize = 12;
const DATA_COLUMNS: usize = 188;
const PARITY_COLUMNS: usize = 16;
const TABLE_BYTES: usize = ROWS * DATA_COLUMNS;
const PARITY_BYTES: usize = ROWS * PARITY_COLUMNS;
const FEC_PACKETS: usize = 9;
const FEC_DATA_BYTES: usize = 22;

/// Packet length in bytes from the first header byte (table 6).
pub fn packet_length(first_byte: u8) -> usize {
    PACKET_UNIT * (usize::from(first_byte >> 6) + 1)
}

/// Address field of a packet or FEC packet header.
pub fn packet_address(header: &[u8]) -> u16 {
    (u16::from(header[0] & 0x03) << 8) | u16::from(header[1])
}

/// A 24-byte padding packet: address 0, no useful data, valid CRC.
pub fn padding_packet() -> [u8; PACKET_UNIT] {
    let mut packet = [0u8; PACKET_UNIT];
    let crc = crc16(&packet[..PACKET_UNIT - 2]);
    packet[PACKET_UNIT - 2..].copy_from_slice(&crc.to_be_bytes());
    packet
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PacketError {
    Empty,
    Truncated { offset: usize, length: usize },
    UsefulLength { offset: usize, useful: usize },
    Crc { offset: usize, address: u16 },
    ReservedAddress { offset: usize, address: u16 },
}

impl fmt::Display for PacketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "no packets"),
            Self::Truncated { offset, length } => {
                write!(f, "{length}-byte packet at offset {offset} is truncated")
            }
            Self::UsefulLength { offset, useful } => write!(
                f,
                "packet at offset {offset} declares {useful} useful bytes, more than its data field"
            ),
            Self::Crc { offset, address } => {
                write!(
                    f,
                    "packet at offset {offset} (address {address}) has a bad CRC"
                )
            }
            Self::ReservedAddress { offset, address } => write!(
                f,
                "packet at offset {offset} uses address {address}, reserved for FEC packets"
            ),
        }
    }
}

impl std::error::Error for PacketError {}

/// Summary of a validated packet stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketScan {
    pub packets: usize,
    /// Addresses used, excluding padding.
    pub addresses: BTreeSet<u16>,
}

/// Check that `data` is a sequence of whole packets with valid CRCs.
pub fn scan_packets(data: &[u8]) -> Result<PacketScan, PacketError> {
    if data.is_empty() {
        return Err(PacketError::Empty);
    }
    let mut offset = 0;
    let mut packets = 0;
    let mut addresses = BTreeSet::new();
    while offset < data.len() {
        let length = packet_length(data[offset]);
        let Some(packet) = data.get(offset..offset + length) else {
            return Err(PacketError::Truncated { offset, length });
        };
        let useful = usize::from(packet[2] & 0x7f);
        if useful > length - 5 {
            return Err(PacketError::UsefulLength { offset, useful });
        }
        let address = packet_address(packet);
        let crc = u16::from_be_bytes([packet[length - 2], packet[length - 1]]);
        if crc16(&packet[..length - 2]) != crc {
            return Err(PacketError::Crc { offset, address });
        }
        if address == FEC_ADDRESS {
            return Err(PacketError::ReservedAddress { offset, address });
        }
        if address != PADDING_ADDRESS {
            addresses.insert(address);
        }
        packets += 1;
        offset += length;
    }
    Ok(PacketScan { packets, addresses })
}

/// GF(2^8) with p(x) = x^8 + x^4 + x^3 + x^2 + 1 and the RS(204,188) generator
/// g(x) = (x + λ^0)(x + λ^1)...(x + λ^15), λ = 2.
struct Galois {
    exp: [u8; 512],
    log: [u8; 256],
    /// Generator coefficients, `generator[k]` for x^k; `generator[16]` is 1.
    generator: [u8; PARITY_COLUMNS + 1],
}

impl Galois {
    fn get() -> &'static Self {
        static FIELD: OnceLock<Galois> = OnceLock::new();
        FIELD.get_or_init(|| {
            let mut exp = [0u8; 512];
            let mut log = [0u8; 256];
            let mut value = 1u16;
            for (power, slot) in exp.iter_mut().enumerate().take(255) {
                *slot = value as u8;
                log[value as usize] = power as u8;
                value <<= 1;
                if value & 0x100 != 0 {
                    value ^= 0x11d;
                }
            }
            for power in 255..512 {
                exp[power] = exp[power - 255];
            }
            let mut field = Galois {
                exp,
                log,
                generator: [0; PARITY_COLUMNS + 1],
            };
            let mut generator = vec![1u8];
            for root in 0..PARITY_COLUMNS {
                // Multiply by (x + λ^root), coefficients from x^0 upwards.
                let alpha = field.exp[root];
                let mut next = vec![0u8; generator.len() + 1];
                for (k, &coefficient) in generator.iter().enumerate() {
                    next[k + 1] ^= coefficient;
                    next[k] ^= field.mul(coefficient, alpha);
                }
                generator = next;
            }
            field.generator.copy_from_slice(&generator);
            field
        })
    }

    fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            0
        } else {
            self.exp[usize::from(self.log[usize::from(a)]) + usize::from(self.log[usize::from(b)])]
        }
    }
}

/// The 16 parity bytes of the shortened RS(204,188) codeword for `row`, in
/// transmission order. The 51 leading zeros of the shortening leave the
/// remainder unchanged, so they are not fed in.
pub fn rs_parity(row: &[u8; DATA_COLUMNS]) -> [u8; PARITY_COLUMNS] {
    let field = Galois::get();
    let mut parity = [0u8; PARITY_COLUMNS];
    for &byte in row {
        let feedback = byte ^ parity[0];
        parity.copy_within(1.., 0);
        parity[PARITY_COLUMNS - 1] = 0;
        if feedback != 0 {
            for (j, slot) in parity.iter_mut().enumerate() {
                *slot ^= field.mul(feedback, field.generator[PARITY_COLUMNS - 1 - j]);
            }
        }
    }
    parity
}

/// Builds the FEC frame of clause 5.3.5 from the packets passed through it.
pub struct FecEncoder {
    table: Box<[u8; TABLE_BYTES]>,
    filled: usize,
    /// RS Data Table in transmission order, and the next FEC packet to send.
    pending: Option<(Box<[u8; PARITY_BYTES]>, usize)>,
}

impl Default for FecEncoder {
    fn default() -> Self {
        Self {
            table: Box::new([0; TABLE_BYTES]),
            filled: 0,
            pending: None,
        }
    }
}

impl FecEncoder {
    /// Bytes left in the Application Data Table; 0 while FEC packets are due.
    pub fn table_space(&self) -> usize {
        if self.pending.is_some() {
            0
        } else {
            TABLE_BYTES - self.filled
        }
    }

    /// Record a packet sent in the sub-channel. It must fit [`table_space`].
    ///
    /// [`table_space`]: FecEncoder::table_space
    pub fn push_packet(&mut self, packet: &[u8]) {
        assert!(
            packet.len() <= self.table_space(),
            "packet overruns FEC frame"
        );
        for &byte in packet {
            // Column by column: consecutive bytes go down the rows.
            let row = self.filled % ROWS;
            let column = self.filled / ROWS;
            self.table[row * DATA_COLUMNS + column] = byte;
            self.filled += 1;
        }
        if self.filled == TABLE_BYTES {
            let mut parity_table = [[0u8; PARITY_COLUMNS]; ROWS];
            for (row, parity) in parity_table.iter_mut().enumerate() {
                let data = self.table[row * DATA_COLUMNS..(row + 1) * DATA_COLUMNS]
                    .try_into()
                    .expect("row length");
                *parity = rs_parity(data);
            }
            // The RS Data Table is also read out column by column.
            let mut parity = Box::new([0u8; PARITY_BYTES]);
            for (index, slot) in parity.iter_mut().enumerate() {
                *slot = parity_table[index % ROWS][index / ROWS];
            }
            self.pending = Some((parity, 0));
        }
    }

    /// The next FEC packet, once the table is full.
    pub fn next_fec_packet(&mut self) -> Option<[u8; PACKET_UNIT]> {
        let (parity, counter) = self.pending.as_mut()?;
        let mut packet = [0u8; PACKET_UNIT];
        // Packet length 00, 4-bit counter, address 1022.
        packet[0] = ((*counter as u8) << 2) | (FEC_ADDRESS >> 8) as u8;
        packet[1] = FEC_ADDRESS as u8;
        let start = *counter * FEC_DATA_BYTES;
        let end = (start + FEC_DATA_BYTES).min(PARITY_BYTES);
        packet[2..2 + end - start].copy_from_slice(&parity[start..end]);
        *counter += 1;
        if *counter == FEC_PACKETS {
            self.pending = None;
            self.filled = 0;
        }
        Some(packet)
    }
}

/// A source of packets for one sub-channel.
pub trait PacketSource {
    /// The next packet, without consuming it; `None` when no data is available.
    fn peek(&mut self) -> Option<&[u8]>;
    /// Consume the packet returned by the last [`peek`](PacketSource::peek).
    fn advance(&mut self);
}

/// Packets of one validated buffer, repeated forever. A replacement buffer
/// takes over at the next wrap of the current one, so a data group is never
/// cut; at a wrap or without a current buffer, it takes over at once.
#[derive(Default)]
pub struct LoopingPackets {
    current: Option<Arc<Vec<u8>>>,
    offset: usize,
    next: Option<Option<Arc<Vec<u8>>>>,
}

impl LoopingPackets {
    /// Queue `content` (packets already checked by [`scan_packets`]), or
    /// `None` to stop sending data at the next wrap.
    pub fn replace(&mut self, content: Option<Arc<Vec<u8>>>) {
        if self.current.is_none() || self.offset == 0 {
            self.current = content;
            self.offset = 0;
            self.next = None;
        } else {
            self.next = Some(content);
        }
    }

    pub fn has_data(&self) -> bool {
        self.current.is_some()
    }
}

impl PacketSource for LoopingPackets {
    fn peek(&mut self) -> Option<&[u8]> {
        let content = self.current.as_ref()?;
        let length = packet_length(content[self.offset]);
        Some(&content[self.offset..self.offset + length])
    }

    fn advance(&mut self) {
        let Some(content) = &self.current else {
            return;
        };
        self.offset += packet_length(content[self.offset]);
        if self.offset >= content.len() {
            self.offset = 0;
            if let Some(next) = self.next.take() {
                self.current = next;
            }
        }
    }
}

/// Assembles logical frames for an FEC-protected packet mode sub-channel.
#[derive(Default)]
pub struct PacketMultiplexer {
    fec: FecEncoder,
}

impl PacketMultiplexer {
    /// One logical frame of `size` bytes, a multiple of 24. A packet that does
    /// not fit the rest of the frame or of the FEC frame waits, and padding
    /// packets fill the gap.
    pub fn frame(&mut self, size: usize, source: &mut impl PacketSource) -> Vec<u8> {
        debug_assert!(size.is_multiple_of(PACKET_UNIT));
        let mut out = Vec::with_capacity(size);
        while out.len() < size {
            if let Some(packet) = self.fec.next_fec_packet() {
                out.extend_from_slice(&packet);
                continue;
            }
            let room = (size - out.len()).min(self.fec.table_space());
            let start = out.len();
            match source.peek() {
                Some(packet) if packet.len() <= room => {
                    out.extend_from_slice(packet);
                    source.advance();
                }
                _ => out.extend_from_slice(&padding_packet()),
            }
            self.fec.push_packet(&out[start..]);
        }
        out
    }
}

/// Check a sub-channel stream produced by [`PacketMultiplexer`]: every
/// packet CRC, the FEC packet headers, and that each row of every complete
/// FEC frame is an RS(204,188) codeword, by evaluating its syndromes rather
/// than re-encoding. Returns the number of complete FEC frames.
pub fn verify_fec_stream(stream: &[u8]) -> Result<usize, String> {
    let field = Galois::get();
    let mut offset = 0;
    let mut frames = 0;
    while offset < stream.len() {
        let mut table = Vec::with_capacity(TABLE_BYTES);
        while table.len() < TABLE_BYTES {
            let Some(&first) = stream.get(offset) else {
                return Ok(frames);
            };
            let length = packet_length(first);
            let packet = stream
                .get(offset..offset + length)
                .ok_or_else(|| format!("truncated packet at {offset}"))?;
            scan_packets(packet).map_err(|err| format!("at {offset}: {err}"))?;
            table.extend_from_slice(packet);
            offset += length;
        }
        if table.len() != TABLE_BYTES {
            return Err(format!("FEC frame overrun before {offset}"));
        }
        let mut parity = Vec::with_capacity(PARITY_BYTES);
        for counter in 0..FEC_PACKETS {
            let Some(packet) = stream.get(offset..offset + PACKET_UNIT) else {
                return Ok(frames);
            };
            if packet_address(packet) != FEC_ADDRESS
                || packet[0] >> 6 != 0
                || usize::from((packet[0] >> 2) & 0x0f) != counter
            {
                return Err(format!("bad FEC packet header at {offset}"));
            }
            parity.extend_from_slice(&packet[2..]);
            offset += PACKET_UNIT;
        }
        if parity[PARITY_BYTES..].iter().any(|&b| b != 0) {
            return Err(format!("FEC padding is not zero before {offset}"));
        }
        for row in 0..ROWS {
            let codeword = (0..DATA_COLUMNS)
                .map(|column| table[column * ROWS + row])
                .chain((0..PARITY_COLUMNS).map(|column| parity[column * ROWS + row]));
            let codeword: Vec<u8> = codeword.collect();
            for root in 0..PARITY_COLUMNS {
                let alpha = field.exp[root];
                let syndrome = codeword
                    .iter()
                    .fold(0u8, |acc, &byte| field.mul(acc, alpha) ^ byte);
                if syndrome != 0 {
                    return Err(format!("FEC frame {frames} row {row}: nonzero syndrome"));
                }
            }
        }
        frames += 1;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data packet with the given length class, address and payload byte.
    fn packet(length: usize, address: u16, fill: u8) -> Vec<u8> {
        let mut packet = vec![fill; length];
        packet[0] = (((length / PACKET_UNIT - 1) as u8) << 6) | 0x30 | (address >> 8) as u8;
        packet[1] = address as u8;
        packet[2] = (length - 5) as u8;
        let crc = crc16(&packet[..length - 2]);
        packet[length - 2..].copy_from_slice(&crc.to_be_bytes());
        packet
    }

    #[test]
    fn padding_packet_matches_the_reference_crc() {
        // ODR-DabMux writes 22 zero bytes followed by 0x604B.
        assert_eq!(&padding_packet()[22..], &[0x60, 0x4b]);
        assert!(scan_packets(&padding_packet())
            .unwrap()
            .addresses
            .is_empty());
    }

    #[test]
    fn scan_reports_addresses_and_rejects_damage() {
        let mut data = packet(24, 1, 0xaa);
        data.extend(packet(96, 7, 0x55));
        let scan = scan_packets(&data).unwrap();
        assert_eq!(
            (scan.packets, scan.addresses.into_iter().collect::<Vec<_>>()),
            (2, vec![1, 7])
        );

        let mut damaged = data.clone();
        damaged[30] ^= 1;
        assert_eq!(
            scan_packets(&damaged),
            Err(PacketError::Crc {
                offset: 24,
                address: 7
            })
        );
        assert_eq!(
            scan_packets(&data[..100]),
            Err(PacketError::Truncated {
                offset: 24,
                length: 96
            })
        );
        assert!(matches!(
            scan_packets(&packet(24, FEC_ADDRESS, 0)),
            Err(PacketError::ReservedAddress { .. })
        ));
    }

    #[test]
    fn generator_has_the_specified_roots() {
        let field = Galois::get();
        assert_eq!(field.generator[PARITY_COLUMNS], 1);
        for root in 0..PARITY_COLUMNS {
            let alpha = field.exp[root];
            let value = field
                .generator
                .iter()
                .rev()
                .fold(0u8, |acc, &c| field.mul(acc, alpha) ^ c);
            assert_eq!(value, 0, "λ^{root}");
        }
    }

    #[test]
    fn fec_frame_follows_94_data_packets_and_verifies() {
        let mut source = LoopingPackets::default();
        let content: Vec<u8> = (0..10u8).flat_map(|n| packet(24, 1, n)).collect();
        source.replace(Some(Arc::new(content)));
        let mut mux = PacketMultiplexer::default();
        let mut stream = Vec::new();
        for _ in 0..(94 + 9) * 2 {
            stream.extend(mux.frame(24, &mut source));
        }
        for (index, packet) in stream.chunks(24).enumerate() {
            let fec = packet_address(packet) == FEC_ADDRESS;
            assert_eq!(fec, index % 103 >= 94, "packet {index}");
        }
        assert_eq!(verify_fec_stream(&stream), Ok(2));

        stream[5] ^= 0x01;
        assert!(verify_fec_stream(&stream).is_err());
    }

    #[test]
    fn packets_wait_for_room_in_the_frame_and_the_fec_frame() {
        // 96-byte packets in a 48-byte frame never fit: padding only.
        let mut source = LoopingPackets::default();
        source.replace(Some(Arc::new(packet(96, 3, 1))));
        let mut mux = PacketMultiplexer::default();
        let frame = mux.frame(48, &mut source);
        assert!(frame
            .chunks(24)
            .all(|p| packet_address(p) == PADDING_ADDRESS));

        // In a 96-byte frame, a 96-byte packet fits until the FEC frame has
        // fewer than 96 bytes left (2256 = 23 × 96 + 48), then padding fills it.
        let mut mux = PacketMultiplexer::default();
        let mut stream = Vec::new();
        for _ in 0..30 {
            stream.extend(mux.frame(96, &mut source));
        }
        assert_eq!(verify_fec_stream(&stream).unwrap(), 1);
        assert_eq!(packet_address(&stream[23 * 96..]), PADDING_ADDRESS);
        assert_eq!(packet_address(&stream[23 * 96 + 48..]), FEC_ADDRESS);
    }

    #[test]
    fn replacement_content_takes_over_at_the_wrap() {
        let mut source = LoopingPackets::default();
        assert!(source.peek().is_none());
        source.replace(Some(Arc::new(
            [packet(24, 1, 1), packet(24, 1, 2)].concat(),
        )));
        assert_eq!(source.peek().unwrap()[3], 1, "first content starts at once");
        source.advance();
        source.replace(Some(Arc::new(
            [packet(24, 2, 9), packet(24, 2, 9)].concat(),
        )));
        assert_eq!(source.peek().unwrap()[3], 2, "old content until the wrap");
        source.advance();
        assert_eq!(packet_address(source.peek().unwrap()), 2);
        source.advance();
        source.replace(None);
        assert_eq!(packet_address(source.peek().unwrap()), 2);
        source.advance();
        assert!(source.peek().is_none());
        source.replace(Some(Arc::new(packet(24, 3, 0))));
        assert_eq!(
            packet_address(source.peek().unwrap()),
            3,
            "at a wrap, at once"
        );
    }
}

/// Byte-exact comparison with ODR-DabMux v5.5.1, which produced the reference
/// from the same packets (`devsupport/cpp-reference/README.md`).
#[cfg(test)]
mod reference_tests {
    use super::*;

    #[test]
    fn matches_odr_dabmux_enhanced_packet_output() {
        let input = include_bytes!("../tests/fixtures/packet/spi-head.bin");
        let reference = include_bytes!("../tests/fixtures/packet/spi-head.odr-dabmux-subch30.bin");
        scan_packets(input).unwrap();
        let mut source = LoopingPackets::default();
        source.replace(Some(Arc::new(input.to_vec())));
        let mut mux = PacketMultiplexer::default();
        let mut stream = Vec::new();
        while stream.len() < reference.len() {
            stream.extend(mux.frame(24, &mut source));
        }
        // 401 frames: three FEC frames and a wrap of the 300-packet input.
        let first_difference = stream.iter().zip(reference).position(|(a, b)| a != b);
        assert_eq!(first_difference, None, "first differing byte");
        assert_eq!(verify_fec_stream(reference), Ok(3));
    }

    /// Compare with a longer ODR-DabMux capture, for example a full SPI file:
    /// `PACKET_INPUT=spi.bin PACKET_REFERENCE=subch.bin cargo test -- --ignored`.
    #[test]
    #[ignore = "needs PACKET_INPUT and PACKET_REFERENCE files"]
    fn matches_odr_dabmux_capture_from_environment() {
        let read = |name: &str| {
            let path = std::env::var(name).unwrap_or_else(|_| panic!("{name} not set"));
            std::fs::read(&path).unwrap_or_else(|err| panic!("{path}: {err}"))
        };
        let (input, reference) = (read("PACKET_INPUT"), read("PACKET_REFERENCE"));
        let size: usize = std::env::var("PACKET_FRAME_BYTES").map_or(24, |v| v.parse().unwrap());
        scan_packets(&input).unwrap();
        let mut source = LoopingPackets::default();
        source.replace(Some(Arc::new(input)));
        let mut mux = PacketMultiplexer::default();
        let mut stream = Vec::new();
        while stream.len() < reference.len() {
            stream.extend(mux.frame(size, &mut source));
        }
        let first_difference = stream.iter().zip(&reference).position(|(a, b)| a != b);
        assert_eq!(first_difference, None, "first differing byte");
        eprintln!(
            "{} bytes identical, {} FEC frames verified",
            reference.len(),
            verify_fec_stream(&reference).unwrap()
        );
    }
}
