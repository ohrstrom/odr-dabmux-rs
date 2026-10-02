# Verification with etisnoop

[etisnoop](https://github.com/Opendigitalradio/etisnoop) decodes raw ETI and reports FIG contents, FIG repetition rates and DAB+ audio. It is built locally in `__ref/etisnoop/`, which is untracked. The Rust mux emits EDI only, so captures are converted to ETI first.

## Capture and convert

```shell
cd dabmux
cargo build --release
../target/release/dabmux --config config.mux-zh.example.yaml &

nc 127.0.0.1 8850 > /tmp/zh.edi        # TCP EDI output; stop with Ctrl-C after ~30 s
cargo run --release --example edi2eti -- /tmp/zh.edi /tmp/zh.eti
```

[`examples/edi2eti.rs`](../../dabmux/examples/edi2eti.rs) rebuilds 6144-byte ETI(NI) frames from the AF packets with the mux's own frame assembly. TIST is not carried over.

To run several configs at once, or next to a running mux, copy each config and shift its input and output ports.

## Checks

```shell
E=../__ref/etisnoop/etisnoop

$E -i /tmp/zh.eti -v > /tmp/zh.yaml            # full decode
grep -c 'CRC: OK' /tmp/zh.yaml                  # every FIB, 3 per frame
grep -ic 'CRC: \(error\|fail\)' /tmp/zh.yaml    # expect 0

$E -i /tmp/zh.eti -R | grep CAROUSEL | tail -20 # FIG rates, frames per FIG
$E -i /tmp/zh.eti -v -F 0/21                    # one FIG type only

$E -i /tmp/audio.eti -s /tmp/stats.yaml         # DAB+ audio levels per service
$E -i /tmp/audio.eti -d 1                       # subchannel 1 to stream-1.wav
```

The last `CAROUSEL` block covers the whole capture. Per FIG, it gives the average number of frames between FIGs, then between complete database cycles. Requirements in frames, at 24 ms per frame:

| FIGs | Required | Measured 2026-10-02 |
| --- | --- | --- |
| MCI: 0/1, 0/2, 0/3, 0/14 | complete every 4 (96 ms) | 4.00 |
| 0/5, 0/8, 0/9, 0/13, 0/17, labels 1/x | 41 (1 s) | 5–16; labels 38–39 |
| 0/6, 0/21, 0/24 | 416 (10 s) | 5–30 |

## etisnoop quirks

- **FIG 0/20:** reported as `FIG 0/20 unknown`; this version has no decoder for it.
- **Primary flag:** `primary: false` appears for every FIG 0/2 component, including ODR-DabMux output. `fig0_2.cpp` prints `primary=true` when the P/S bit is 0. For packet components it also prints the low SCId bits as "SubChannel ID".
- **Older field names:** FIG 0/6 IdLP is shown as `Shd`, and the FIG 0/9 extended sub-field shows an `LTO` (Rfa in EN 300 401 V2.2.1).
- **PTY before FIG 0/9:** a few `unknown international table Id` lines appear until the first FIG 0/9 arrives.
- **End of file:** `Incomplete frame in ETI file!` and `ETI file read error` are printed at the end of every file.
- **No packet mode:** `-d` decodes DAB+ superframes only, not packet mode subchannels. For SPI, use the byte comparison and syndrome check in [devsupport/cpp-reference/](../../devsupport/cpp-reference/README.md).

## Results 2026-10-02

Captured 30 s each of the mux-zh (18 services with SPI), service-linking and 12-service production examples, and 20 s of `config.example.yaml` fed by the devsupport encoders.

- **Frames:** all FIB CRCs are OK. There was one skipped frame (FCT 89 → 91) at the same instant in two parallel captures, a host stall; the mux skips rather than bursts.
- **Signalling:** FIG 0/3, 0/8, 0/13, 0/14, 1/5 (SPI) and 0/9 with the extended ECC field decode as configured. So do FIG 0/6 (TS 103 176 order, ILS derived), 0/21 (FM 87.6 and 105.2 MHz; the C++ capture shows 105.1) and 0/24 (OE = 0 and 1).
- **Audio:** all three DAB+ programmes decoded; subchannel 1 played continuously for 19.9 s with no silent windows.
