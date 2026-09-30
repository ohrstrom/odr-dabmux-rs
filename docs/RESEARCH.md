# ODR-DabMux C++ reference research

The bundled C++ source at `__ref/ODR-DabMux/` is the behavioral reference for the Rust port. The compiled binary `__ref/ODR-DabMux/odr-dabmux --version` reports `v5.5.1-dirty` (checked 2026-09-30). The Rust implementation remains separate. `__ref/sonicecast/` is a Rust hot-reload example. `__ref/edinburgh/` is a Rust EDI receiver/player (GPL-2.0); its EBU Latin table and FIC decoder are reused (see [REVIEW_FINDINGS.md](REVIEW_FINDINGS.md), R1/R2). `docs/etsi/README.md` indexes standards. `__ref/` is not tracked in git. Source paths below are relative to `__ref/ODR-DabMux/`.

## Architecture and control flow

| Subsystem | Source | Role |
| --- | --- | --- |
| Process | `src/DabMux.cpp` | CLI, signals, configuration, outputs, operational services, frame loop. |
| Mux | `src/DabMultiplexer.cpp` | Validation, clock setup, ETI/EDI generation. |
| Model and parser | `src/MuxElements.{h,cpp}`, `src/ConfigParser.cpp` | Ensemble graph, protection, labels, links, INFO/JSON config. |
| FIC | `src/fig/` | FIG encoders and classic/priority schedulers. |
| Inputs | `src/input/` | MPEG/raw files, UDP, ZMQ, EDI, STI, PRBS. |
| Outputs | `src/dabOutput/`, `lib/edioutput/` | ETI sinks and EDI TAG/AF/PFT transport. |
| Operations | `src/ManagementServer.*`, `lib/RemoteControl.*`, `lib/ClockTAI.*` | Stats, remote mutation, TAI offset. |

`main()` reads config, starts management/control and outputs, calls `DabMultiplexer::prepare()`, then repeatedly calls `mux_frame()` (`src/DabMux.cpp:96-569`). Preparation parses the model, constructs a FIG carousel, validates IDs and references, opens inputs, obtains effective bitrates, computes consecutive CU addresses, checks capacity, then initializes time (`src/DabMultiplexer.cpp:164-474`). Every loop iteration creates one 24 ms frame. Output writes are synchronous, although network inputs buffer separately.

The positive `general.nbframes` limit is checked *after* a frame is written against a zero-based counter; N appears to produce N+1 frames (`src/DabMux.cpp:525-531`). `main()` rejects zero conventional outputs, apparently including EDI-only configurations (`src/DabMux.cpp:511-516`). Account for these snapshot behaviors in differential tests.

## Configuration and model

`DabMultiplexerConfig::read()` selects JSON for `.json` filenames and Boost INFO otherwise (`src/DabMultiplexer.cpp:125-142`). Examples are `doc/example.mux`, `doc/advanced.mux`, `doc/servicelinking.mux`, and `doc/example.json`. Core top-level sections: `general`, `ensemble`, `services`, `subchannels`, `components`, `outputs`. Optional sections include `linking`, `frequency_information`, `other-services`, `service-component-information`, and `remotecontrol`. INFO block names are UIDs; numeric IDs are signalled on air (`src/ConfigParser.cpp:482-1025`). Subchannel order matters: start addresses are assigned sequentially (`src/DabMultiplexer.cpp:415-474`).

`general.dabmode` accepts 1–4 (default 1). `general.fic-scheduler` accepts `classic`, `classic-rate-tuning`, or `priority` case-insensitively; unknown names warn and default to classic (`src/fig/FIGSchedulerType.cpp:29-56`). Time options include `tist`, `tist_offset`, `tist_at_fct0`, `tai_clock_offset`, and `tai_clock_bulletins`. Process options include `nbframes`, management/HTTP ports, `syslog`, and `startupcheck`, whose value runs as a shell command (`src/DabMux.cpp:160-270`).

Ensemble settings include ID, ECC, labels, local-time offset, international table, reconfiguration counter, and announcements. The counter may be `hash`, calculated from selected service/component/subchannel fields (`src/DabMultiplexer.cpp:255-296`). Services carry IDs, labels, PTY, language and optional announcements/links. Components resolve service/subchannel UIDs and carry SCIdS, type, packet parameters and optional user applications. Preparation sets DAB+ audio ASCTy to `0x3f`, EEP MPEG audio to `0x00`, and assigns packet component IDs (`src/DabMultiplexer.cpp:317-413`).

Subchannel types are `audio` (MPEG Layer II), `dabplus`, `data`/`dmb`, `packet`, `enhancedpacket`. Modern inputs require both `inputproto` and `inputuri`; legacy `inputfile` infers a protocol (`src/ConfigParser.cpp:1065-1120`). Audio/DAB+ allow file, ZMQ, EDI and STP; data allows UDP or raw file/FIFO; packet uses packet file; PRBS is available for data (`src/ConfigParser.cpp:1120-1213`). The `dmb` path explicitly warns that older DMB interleaving/Reed–Solomon code is unported and uses raw input. Options include `nonblock`, `load_entire_file`, `buffer-management` (`prebuffering`/`timestamped`) and `tist-delay`.

Bitrate is mandatory and must be a multiple of 8 kbit/s; subchannel ID is 0–63 or auto-assigned. MPEG audio defaults to UEP and other types to EEP-A. Protection level is 1–5 for UEP or 1–4 for EEP in config, stored zero-based (`src/ConfigParser.cpp:1249-1345`). UEP survives only if bitrate/level match its table; otherwise it changes to EEP. EEP-B requires bitrate divisible by 32 (`src/DabMultiplexer.cpp:440-472`). MSC bytes per frame = `bitrate * 3`. EEP-A CU sizes for levels 1–4 are `bitrate*12/8`, `bitrate`, `bitrate*6/8`, `bitrate/2`; EEP-B uses `27/32`, `21/32`, `18/32`, `15/32`; UEP uses its lookup table (`src/MuxElements.cpp:828-894`). Capacity is 864 CU. TPL encodes protection form, option and level (`src/MuxElements.cpp:431-440`).

## ETI frame layout

`mux_frame()` zeroes a 6144-byte buffer and writes a variable logical length (`src/DabMultiplexer.cpp:554-937`). Packed ETI structures are in `src/Eti.h`; CRC code is in `lib/crc.c`.

| Section | Reference behavior |
| --- | --- |
| SYNC | ERR=`0xff`; FSYNC alternates a 24-bit constant and its complement by frame parity. |
| FC | FCT=`currentFrame % 250`; FICF=1; NST=subchannel count; FP=`currentFrame & 7`; MID modes I/II/III/IV = 1/2/3/0. |
| STC | Four bytes per subchannel: ID, CU start, TPL and stream length, in config order. |
| EOH | Frame-phase-dependent MNSC time and inverted CRC16 of FC, STC, MNSC. |
| MST | FIC followed by `bitrate*3` bytes from each subchannel. |
| EOF | Inverted CRC16 of MST; RFU=`0xffff`. |
| TIST | 24-bit timestamp plus `0xff`, or all ones when disabled. |

FICL is 24 words for modes I/II/IV, 32 for III. `FL = 1 + FICL + NST + sum(subchannel_bytes/4)` words; logical frame size is `(FL + 4)*4` bytes (`src/DabMultiplexer.cpp:616-920`). The scheduler writes three FIBs (four for mode III), each 30 FIG bytes plus two CRC bytes. It zero-fills unused space, places `0xff` as an end marker when room remains, and uses inverted CRC16 (`src/fig/FIGCarousel.cpp:226-273`, `FIGCarouselPriority.cpp:359-397`). In classic scheduling FIG 0/0 gets first position and FIG 0/7 follows it (`src/fig/FIGCarousel.cpp:367-422`). Scheduler state affects successive frames; a one-frame comparison cannot establish parity.

Read errors are logged without aborting the frame. The zeroed destination allows silence for absent data; adapters may pad, mute, rewind or select timestamped content (`src/DabMultiplexer.cpp:799-823`, `src/input/File.cpp:291-388`). `InputBase::readFrame()` returns `size_t` although comments discuss negative errors; the caller stores it in `int` (`src/input/inputs.h:48-87`). Avoid reproducing this signedness ambiguity in Rust.

## FIC signalling

Classic scheduling uses FIG deadlines decremented by 24 ms/frame; `classic-rate-tuning` adds per-FIG correction factors. Priority scheduling has deadline and urgency passes (`src/fig/FIGCarousel.cpp`, `FIGCarouselPriority.cpp`). FIG 0 extensions with source files are 0, 1, 2, 3, 5–10, 13, 14, 17–21 and 24. FIG1 and FIG2 cover labels. Key roles include ensemble/change (0/0, 0/7), subchannel organization (0/1), service organization (0/2), components/applications (0/3, 0/8, 0/13, 0/14), time/country (0/9, 0/10), announcements (0/18, 0/19), and linking/frequency information (0/6, 0/21, 0/24). Exact emission conditions, bit layouts and rates need to be read in each encoder and carousel when porting; source presence does not imply emission for every configuration.

`reload_linking` re-reads the config, validates linkage sets, frequency information and other-ensemble services independently, then commits all three only if valid. It is a targeted runtime update, not full config hot reload (`src/DabMultiplexer.cpp:475-541`). Other remote parameters mutate selected labels, announcements and service data (`src/MuxElements.cpp`).

## Timing, I/O and operations

`MuxTime::init()` applies `tist_offset` to wall time and aligns the initial 24 ms counter for `tist_at_fct0`. Every frame advances time by 24 ms even with TIST disabled, so startup FCT and FIG phase depend on wall time (`src/DabMultiplexer.cpp:44-123`, `:220-250`). TIST plus EDI, or metadata-enabled ZMQ, requires a TAI–UTC offset at startup. EDI DETI and output metadata receive seconds, UTCO, timestamp and DLFC (`src/DabMultiplexer.cpp:237-254`, `:846-879`). `doc/TIMESTAMPS.rst` explains the post-v2.3.1 offset semantics.

File inputs read directly; MPEG file input parses Layer II frames and mutes/pads damaged or short frames. Timestamped file reads return zero-filled data (`src/input/File.cpp`). UDP, ZMQ and EDI have protocol-specific buffers (`src/input/Udp.cpp`, `Zmq.cpp`, `Edi.cpp`). Effective bitrate may change when `setBitrate()` runs; final CU layout must follow input initialization (`src/DabMultiplexer.cpp:415-472`).

Conventional outputs are file, FIFO, raw device, UDP, TCP, simulator and ZMQ, subject to compile flags. EDI has separate UDP/TCP destinations, TAG alignment and optional PFT/FEC/spreading (`src/DabMux.cpp:291-508`). File formats: `framed` (count plus each length), `streamed` (each length), `raw` (6144 bytes/frame padded with `0x55`); count and lengths are native integers in C++ (`src/dabOutput/dabOutputFile.cpp:35-115`, `doc/dab_output_formats.txt`). ZMQ batches four frames/message and appends metadata (`src/dabOutput/dabOutputZMQ.cpp:69-124`). EDI emits `*ptr`, DETI and one ESTn tag per subchannel (`src/DabMultiplexer.cpp:922-937`, `lib/edioutput/`).

Management exposes stats, FIG missed deadlines and EDI TCP status; optional HTTP, telnet and ZMQ servers expose stats/control. The loop checks control faults every 250 iterations and updates management every ten (`src/DabMux.cpp:535-557`). HUP, QUIT, INT and TERM stop the loop (`src/DabMux.cpp:64-91`).

## Differential validation plan

1. Parse and validate the model, especially UID resolution, effective bitrates, subchannel order and the 864 CU bound.
2. Serialize ETI fields and CRCs with explicit byte order. Compare fixed fields and whole frames against reference output; do not map C++ packed bitfields directly onto Rust structs.
3. Compare multiple consecutive FIBs/frames for each scheduler. Freeze or normalize FCT, MNSC, FIG 0/10 and TIST where wall time affects bytes.
4. Start with PRBS or a constant raw-file input and `file://...?type=raw`; compare the logical frame separately from `0x55` padding.
5. Add EDI, timestamping, network buffering and runtime control after ETI. Test 24 ms/second rollover and the narrow scope of linking reload.

Record binary version and build options with fixtures. The C++ tree declares GPL-3.0-or-later (`COPYING` and source headers), relevant before copying source or tables.
