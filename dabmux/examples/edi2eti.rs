//! Convert a captured EDI stream (AF packets back to back, as served on the
//! TCP output) to raw ETI(NI) frames of 6144 bytes, for tools that read ETI
//! such as etisnoop.
//!
//! ```shell
//! nc 127.0.0.1 9000 > capture.edi        # stop with Ctrl-C
//! cargo run --example edi2eti -- capture.edi capture.eti
//! ```

use anyhow::{bail, Context, Result};
use dabmux::edi::AfPacket;
use dabmux::frame::{assemble, FrameClock, Stream};

const ETI_FRAME_BYTES: usize = 6144;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, input, output] = &args[..] else {
        bail!("usage: edi2eti <capture.edi> <output.eti>");
    };
    let data = std::fs::read(input).with_context(|| format!("reading {input}"))?;
    let mut offset = data
        .windows(2)
        .position(|w| w == b"AF")
        .context("no AF packet in the capture")?;
    let mut eti = Vec::new();
    let mut frames = 0usize;
    while offset + 10 <= data.len() {
        let length = u32::from_be_bytes(data[offset + 2..offset + 6].try_into()?) as usize;
        let end = offset + 12 + length;
        if end > data.len() {
            break; // capture cut off mid-packet
        }
        let packet = AfPacket::decode(&data[offset..end])
            .with_context(|| format!("AF packet at byte {offset}"))?;
        eti.extend(frame(&packet)?);
        frames += 1;
        offset = end;
    }
    std::fs::write(output, &eti).with_context(|| format!("writing {output}"))?;
    eprintln!("{frames} ETI frames written to {output}");
    Ok(())
}

fn frame(packet: &AfPacket) -> Result<Vec<u8>> {
    let deti = packet
        .tags
        .iter()
        .find(|tag| &tag.name == b"deti")
        .context("AF packet without DETI")?;
    let value = &deti.value;
    let header = u16::from_be_bytes([value[0], value[1]]);
    let dlfc = u64::from((header >> 8 & 0x1f) * 250 + (header & 0xff));
    let eti_header = u32::from_be_bytes(value[2..6].try_into()?);
    let mnsc = eti_header as u16;
    let mid = (eti_header >> 22) & 3;
    let mode = match mid {
        1 => 1,
        2 => 2,
        3 => 3,
        _ => 4,
    };
    let timestamp = header & 0x8000 != 0;
    let fic_start = if timestamp { 6 + 8 } else { 6 };
    let fic_len = if mode == 3 { 128 } else { 96 };
    let fic = &value[fic_start..fic_start + fic_len];
    let streams: Vec<Stream> = packet
        .tags
        .iter()
        .filter(|tag| &tag.name[..3] == b"est")
        .map(|tag| {
            let sstc = u32::from_be_bytes([0, tag.value[0], tag.value[1], tag.value[2]]);
            Stream {
                id: (sstc >> 18) as u8 & 0x3f,
                start_address_cu: (sstc >> 8) as u16 & 0x3ff,
                tpl: (sstc >> 2) as u8 & 0x3f,
                payload: &tag.value[3..],
            }
        })
        .collect();
    // FCT and frame phase follow from DLFC, as in the mux.
    let clock = FrameClock::new(dlfc, 0, 0)?;
    let mut bytes = assemble(mode, clock, mnsc, fic, &streams, false)?.bytes;
    bytes.resize(ETI_FRAME_BYTES, 0x55);
    Ok(bytes)
}
