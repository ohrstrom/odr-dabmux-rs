# Rust DAB multiplexer implementation plan

This plan uses the bundled C++ source and binary as a behavioral reference; see [RESEARCH.md](RESEARCH.md). The target is a continuously running, real-time DAB multiplexer with **EDI and STI inputs only** and **EDI output only**. It emits one multiplex frame every 24 ms. There is no selectable offline or unpaced operating mode, ETI file output, ZMQ path, or one-to-one port of the C++ management stack. ETI remains an internal frame representation where useful for constructing and checking EDI.

The supplied 12-service production configuration is audited separately in [PRODUCTION_CONFIG_GAP.md](PRODUCTION_CONFIG_GAP.md). Its PTY, language, slideshow and timing requirements take priority over unrelated C++ features.

## Current state and architecture

The `dabmux` crate has a validated YAML model, EDI and STI receivers, a 24 ms frame loop, audio FIC carousel, EDI UDP/TCP output, HTTP stats, and full configuration hot reload. A code review against the C++ reference and the ETSI standards found 14 issues. All are resolved, and one is mitigated; see [REVIEW_FINDINGS.md](REVIEW_FINDINGS.md). The FIG writers are now checked against an independent decoder vendored from the EDInburgh receiver. The remaining gates below are interoperability, resilience and operational hardening work; see the progress log for evidence and limits.

Keep five boundaries clear:

1. **Config and validation:** parse native config, resolve UIDs and defaults, and construct an ordered `ValidatedEnsemble` before changing a running mux.
2. **Input receivers:** receive EDI or STI, validate their framing and timestamps, buffer bounded amounts of subchannel data, and report availability for a requested frame time.
3. **Frame engine:** own the 24 ms clock, FIC scheduler and ordered MSC assembly. Serialize ETI-compatible fields internally as needed for reference comparison and EDI DETI/ESTn construction.
4. **EDI sender:** encode TAG/AF and selected PFT/FEC transport, send to configured destinations, and report transport health.
5. **Control plane:** expose truthful stats via API and accept config updates from file reload or API push. Validate a complete candidate before committing it at a transmission-frame boundary.

Use explicit byte serialization, checked lengths/arithmetic and no packed Rust bitfields or `unsafe`. Keep the real-time scheduler separate from deterministic frame logic so tests can inject time without adding a non-real-time product mode. Preserve subchannel order: it determines CU addresses, STC/MSC order and EDI ESTn mapping. Define bounded queues and an explicit missing/late input policy. Any work that might block on I/O or config parsing must stay off the 24 ms frame path.

## Phase 0 — reference and protocol fixtures

**Work:** Build a test harness around `__ref/ODR-DabMux/odr-dabmux` (`v5.5.1-dirty`) and record its build/version, config, inputs, timestamps and expected outputs. Reference ETI files may be used **inside tests** to inspect FC/STC/FIB/MST/CRC fields; they are not a Rust output feature. Capture or synthesize representative EDI and STI packets, including valid, truncated, out-of-order and timestamped cases. Use independent decoders where possible. Normalize clock-dependent FCT, MNSC, FIG 0/10 and TIST before byte comparisons.

**Gate:** Fixtures can be decoded and checked for frame boundaries, payload lengths, timestamps and CRCs. The harness can compare multi-frame reference behavior reproducibly.

## Phase 1 — native config and validated ensemble

**Work:** Replace the placeholder YAML service list with ensemble, service, component, subchannel, EDI/STI input and EDI destination definitions. YAML can remain the native format; C++ INFO/JSON import is optional tooling, not a runtime requirement. Validate unique UIDs/IDs, references, labels, mode, bitrate/protection combinations and the 864 CU bound. Represent UEP, EEP-A/B, TPL and CU size explicitly. Resolve effective bitrate and sequential start addresses before activating the ensemble. Reject unsupported input/output types and unsupported active signalling options clearly.

**Gate:** Table-driven tests cover a valid multi-service ensemble, reordered subchannels, protection levels, duplicate IDs, broken links, invalid bitrate and CU overflow. Invalid config cannot alter the active mux or open an EDI destination.

## Phase 2 — 24 ms clock and internal frame assembly

**Work:** Implement an injected frame clock with CIF count, FCT, frame phase, MNSC, TIST and seconds/24 ms offset. Serialize SYNC, FC, STC, EOH, MST boundaries, EOF and TIST into a bounded internal frame buffer, with explicit endian handling and CRC16. Calculate FICL, FL and subchannel payload slices for modes I–IV. Tests can use zero-filled FIC/MSC; that is a test fixture, not valid output. Define how frame deadlines relate to input timestamps and when a frame is considered late.

**Gate:** Every field and CRC decodes correctly across parity, FCT/FP rollover, second rollover, mode III and near-capacity layouts. Controlled internal frames compare with the reference after accounting for time-dependent fields.

## Phase 3 — minimum useful FIC

**Work:** Implement FIG/FIB writers, then ensemble/change, subchannel, basic service/component and label signalling (initially FIG 0/0, 0/1, 0/2, 0/7 as needed, and FIG 1). Add the classic carousel with 30-byte FIB payload bounds, end marker, padding, CRC and persistent deadlines. Include time/country and DAB+ application signals needed for the first supported ensemble. Keep individual FIG encoders testable without the frame loop.

**Gate:** An independent decoder reconstructs the configured ensemble, services, components and subchannels across consecutive frames. FIB placement and repetition are compared over many frames, not just one.

## Phase 4 — EDI and STI input receivers

**Work:** Implement EDI input decoding and STI input decoding as separate adapters that deliver validated, timed subchannel payloads to the frame engine. Define transport endpoints actually supported by each adapter. Check stream identity, bitrate/length, CRC and timestamp information; reject incompatible input data. Use bounded jitter buffers, sequence tracking and a documented policy for missing, early, late, duplicate and disconnected frames. Select data for the mux frame timestamp; underflow must produce a deliberate mute/fill result plus a metric, never silently reuse stale bytes. Keep protocol reception asynchronous and frame selection nonblocking.

**Gate:** Recorded and loopback EDI/STI fixtures feed correct MSC slices into consecutive frames. Tests cover jitter, loss, duplicate/out-of-order packets, timestamp rollover, reconnect and bounded memory. Both input types work in one ensemble.

## Phase 5 — EDI output and real-time engine

**Work:** Encode `*ptr`, DETI and ESTn tags from the frame engine, then TAG packet and AF; implement the configured EDI transport and packetisation. Validate TIST/MNSC/EDI timestamp alignment with an explicit TAI–UTC override when timestamps are enabled. Start the frame scheduler as the application's only operating mode: one frame every 24 ms, regardless of input availability. Establish startup alignment, bounded output queues, deadline/overrun policy, send failure behavior and clean shutdown. There is no optional throttle, burst mode or ETI sink. A mock sender and injected clock provide deterministic tests without changing production cadence.

**Gate:** An independent EDI decoder/receiver accepts output and reconstructs FIC and each ESTn payload. Sustained loopback tests show 24 ms cadence, bounded memory and defined behavior under input loss, slow destinations and clock rollover. Compare decoded output semantics with the C++ reference.

## Phase 6 — supported signalling and scheduling breadth

**Work:** Add only FIGs needed by the supported ensemble configuration: component/application details, announcements, service linking, frequency information, other-ensemble services, extended labels and packet/data signalling as those features become configurable. Add classic rate tuning or priority scheduling only if required by signalling load or target behavior. Validate each feature's config and wire encoding together, and measure actual repetition deadlines. Keep unsupported C++ features explicit rather than carrying unused encoders.

**Gate:** Every exposed config feature has a decoded signalling fixture and multi-frame repetition check. Loaded ensembles meet the chosen FIG deadline policy or report a clear capacity/config error.

## Phase 7 — API stats and atomic config updates

**Work:** Replace config-echo endpoints with runtime stats: generated frames, timing misses, input buffer depth/underflow/loss, FIG deadline misses, output queue depth and EDI send failures. Use the file watcher and complete-config API push for updates. Both paths parse and validate a complete candidate, prepare changed sockets without disturbing the active mux, and activate all ensemble, service, component, subchannel, input and output changes together at a frame boundary. Failed validation or resource preparation leaves the previous ensemble active. Do not port C++ telnet/ZMQ remote control or management server protocols.

**Gate:** API stats match observed frame/input/output events; file reload and API push produce the same validation and update semantics. Invalid pushes, partial file writes and failed activation preserve the running mux. Tests verify the frame boundary at which each accepted change takes effect.

## Release evidence

At each phase, use unit tests for calculations, independent protocol decoding, and differential multi-frame comparison with the bundled C++ reference. Compare field semantics first; require exact bytes only when time, scheduler state and input packets are controlled. Run formatting, linting and tests at each milestone. Before deployment, exercise mixed EDI/STI inputs, missing input, destination outage, timestamp rollover, high CU use, long-running cadence and config updates during active output.

Track each supported feature as **implemented**, **reference compared** and **interoperability verified**. Release readiness requires a valid continuously emitted EDI multiplex, documented failure behavior, truthful API stats and safe config updates. The C++ tree declares GPL-3.0-or-later; resolve licensing before copying source or tables. The EBU Latin table in `dabmux/src/charset.rs` and the test-only decoder in `dabmux/src/testsupport/edinburgh/` come from the GPL-2.0 EDInburgh project. Confirm with its author that it is licensed "or later" before distribution.

## Progress log

### 2026-09-30 — configuration foundation (Phase 1, partial)

- Replaced the placeholder service-only YAML with ensemble, service, component, ordered subchannel, EDI/STI input and EDI destination definitions. Updated `dabmux/config.example.yaml`.
- Added pre-activation validation for mode, required sections, unique UIDs/IDs, component references, service components, EEP-A/B level and bitrate, sequential CU addresses, 864 CU capacity, payload sizes and ETI frame length. The validated model preserves subchannel order and computed TPL/size.
- Startup now requires `--config`; file reload parses and validates a complete candidate before replacing the shared config. The stats endpoint states that no mux runtime statistics exist yet instead of reporting config as runtime data.
- Added configuration tests for a two-subchannel layout, EEP protection, invalid references/capacity and rejection of invalid candidates. `cargo fmt --all`, `cargo test --workspace` (4 tests) and `cargo clippy --workspace --all-targets -- -D warnings` pass.

**Still open in Phase 1:** input transport URI validation, actual receiver-derived effective bitrate, label charset/short-label rules, UEP support if required, deeper component/service signalling fields and output destination resource preparation. The current watcher still replaces shared validated config directly; frame-boundary activation cannot be implemented until the frame engine exists. The application remains an API scaffold and does not emit EDI yet.

**Next:** Phase 0 reference/protocol fixtures and the deterministic clock/frame serializer from Phase 2. Keep the production path fixed at one frame every 24 ms when the engine is introduced.

### 2026-09-30 — first running EDI path (Phases 0–5 and 7, partial)

- Added a deterministic frame clock, ETI field/CRC serializer and a minimum FIC carousel for programme audio (FIG 0/0, 0/1, 0/2 and FIG 1 labels). A captured C++ frame exposed and then verified the STC/FIG 0/1 bit packing. The C++ fixture emitted five frames for `nbframes 4`, confirming the off-by-one behavior noted in research.
- Added bounded AF/TAG and non-FEC PFT encoding/decoding, DETI/ESTn output tags, EDI/DSTI input extraction and STI-D/RTP payload extraction. UDP and TCP EDI inputs and outputs are wired; STI-D/RTP uses UDP. The active loop emits one frame per 24 ms timer tick and substitutes zeros for missing input.
- Added basic runtime counters to `/api/stats`, JSON `POST /api/config`, and validated file reload. Only ensemble/service labels can change live; layout, input, output and timing changes return a restart-required error. Config is read once per frame, so an accepted label update takes effect on a subsequent frame.
- Added optional TIST/EDI absolute timestamp fields with an explicit `tai_utc_offset` requirement, plus UTC MNSC calendar encoding.
- Local loopback smoke checks observed 23.4–25.4 ms intervals across eight UDP packets, correct stats, EDI/DSTI payload forwarding over UDP and TCP, and a valid TCP EDI stream. `cargo test --workspace` (13 tests) and strict Clippy pass.

This entry recorded the state at that milestone; the next entry supersedes its remaining-work list.

### 2026-09-30 — interoperability and clock alignment

- Added FIG 0/7 (optional reconfiguration counter), FIG 0/9 (ECC, local time offset and international table), and FIG 0/10 (UTC). The audio carousel packs several FIG 0/2 service entries per FIB, rotates labels, and emits CRC-protected FIBs.
- Added prebuffered EDI reception and timestamp matching, with bounded queues and counters for drops, late frames and invalid timestamps. Synthetic live loopback verified EDI/DSTI over UDP and TCP, STI-D/RTP over UDP, API label push, and rejection of a structural change while output continued.
- Rebuilt the binary and captured its TCP AF output. DABlin v1.16.0-dirty independently decoded the ensemble ID and label, 96 kbit/s EEP 3-A subchannel, DAB+ service and label, ECC/LTO, and UTC date/time. This confirms signalling interoperability for the example ensemble; the zero-filled audio on input underflow cannot be played.
- Aligned startup frame count and timestamp fields to one 24 ms wall-clock instant. Timestamped input now sorts reordered frames and drops duplicate timestamps within its bounded buffer. Regression tests cover both invariants. `cargo test --workspace` passes 19 tests; formatting and strict Clippy checks pass.
- Added Ctrl-C shutdown so the HTTP server, frame loop and owned receiver tasks stop when the application exits.

**Open release gates:** Mixed EDI/STI sustained tests, packet loss/reordering and reconnect behavior, high-load FIG repetition measurements, timestamped on-air input alignment and rollover, output destination outage behavior, and file watcher and API frame-boundary verification remain. PFT FEC, multicast and automatic TAI–UTC lookup are not exposed by the current configuration; add them only if deployment requires them. The supported configuration is programme audio with EEP-A/B, EDI/DSTI or STI-D/RTP inputs, and EDI UDP/TCP output. Other C++ signalling features are outside this configuration's scope.

### 2026-09-30 — complete configuration hot reload

- Replaced the label-only rule with a serialized prepare-and-commit path shared by `POST /api/config` and watched YAML reloads. New input and TCP output sockets bind before activation. A bind or validation failure returns an error and leaves the active mux and its sockets unchanged.
- At the next 24 ms frame tick, the mux swaps the validated ensemble, receiver set, output destinations and FIC carousel together. Unchanged subchannels retain their receiver buffers and TCP output listeners retain connected clients. Changed subchannels start fresh receiver buffers. Removed receiver and TCP output tasks are aborted with their child connections.
- A local HTTP integration check changed ensemble and service IDs, subchannel ID and bitrate, added a service, STI input and output destination, and observed the second EST tag on the existing TCP EDI connection. A deliberately occupied new TCP output port returned HTTP 400; the previous output remained active.
- `/api/stats` exposes a `config_activations` counter so operators can see completed live changes. Unit tests use a fixed fixture instead of the editable example YAML.
- A watched-file check showed that malformed YAML preserves the current service, then an atomic replacement changes its service and subchannel IDs. The watcher now normalizes macOS `/var` and `/private/var` paths when matching filesystem events.
- DABlin independently decoded a newly added 64 kbit/s EEP 3-A subchannel, its service ID and label after a live API update, then selected that service from the post-update EDI stream.

**On-air behavior:** The new FIC and MSC layout begin on one frame boundary. This is an immediate ensemble change; the mux does not yet announce a future ETSI reconfiguration point, so receivers may need to reacquire after structural updates. Verify receiver behavior for this transition before on-air deployment.

### 2026-09-30 — production config audit and explicit short labels

- Compared the supplied 12-service INFO config with `doc/advanced.mux`, the C++ short-label/parser/FIG code and the Rust model. The deployed EDI/TCP audio layout maps to the Rust core and uses 552 of 864 CUs.
- Added optional `short_label` to ensemble and service YAML/JSON config. Validation requires a printable, at most eight-character subsequence of the full label. FIG 1 emits the corresponding 16-bit character mask; omission retains the C++ first-eight default. DABlin independently decoded `RND D00 - XX` → `RND D00` and `105 DJ HRND-001` → `HRND-001` from Rust output.
- Documented missing production fields in `PRODUCTION_CONFIG_GAP.md`: service PTY/language, component slideshow user application, 2-second TIST offset, automatic TAI and local-time offset, and TCP output queue/preroll/TAG alignment. These are concrete deployment gaps, distinct from excluded C++ subsystems.

### 2026-09-30 — production signalling, timing and TCP behavior

- Added service `pty` and `language` and component `user_applications: [slideshow]`; the FIG carousel emits FIG 0/17, 0/5 and 0/13. A 12-service fixture completes all three metadata sets within 48 frames. DABlin decoded PTY, German language and Slideshow from a live 12-service Rust EDI stream.
- Added `tist_offset_ms` and `tist_at_fct0_ms`. Startup aligns FCT to the requested timestamp phase; live TIST offset changes shift the timestamp clock without resetting the frame counter. A DETI loopback check measured a 1.986-second lead for the production 2000 ms setting and approximately zero after a live update.
- Added pipe-separated HTTPS TAI bulletin sources, parsed by effective and expiry dates, with startup resolution and hourly refresh. A fixed `tai_utc_offset` still takes precedence. The last expired bulletin is used with a warning when no current source is available. Added automatic FIG 0/9 local time offset from the host timezone.
- Added per-TCP-client bounded frame queues, configured preroll history and 16-byte TAG alignment via `*dmy`. A new client received 120 buffered frames in about 2 ms in local loopback. The 12-service stream used a 500-frame queue and 3500 ms preroll.
- Added a validated Rust YAML translation of the supplied production INFO config at `dabmux/config.production.example.yaml`. Live testing used a fixed TAI offset; the production HTTPS bulletin URLs and actual audio inputs still require deployment-host verification.

### 2026-09-30 — real ODR-AudioEnc EDI input

- Removed the AF payload multiple-of-eight restriction from the input decoder. AF payloads may end at any byte boundary; individual TAG lengths remain byte aligned. The real ODR-AudioEnc v3.4.0 sends 265-byte AF payloads, which the old check rejected.
- Added a regression test for an unpadded 265-byte AF payload. A local TCP run with the supplied `devsupport/test.wav` and `odr-audioenc -e tcp://127.0.0.1:9001` produced 209 mux frames with zero EDI decode errors. The file encoder ran ahead of the 24 ms mux cadence, filling its input queue and producing drops; verify continuous paced input separately.
- Corrected STI `ssNN` extraction to skip its three-byte ISTC header, matching the reference writer and decoder. The previous four-byte skip made every 72 kbit/s audio frame 215 bytes instead of 216; the mux replaced each with silence despite zero EDI decode errors.
- Re-ran the real ODR-AudioEnc with `ffmpeg -re` pacing raw PCM from `devsupport/test.wav`, captured the mux's TCP EDI output, and checked it with DABlin. In nine seconds, the mux reported zero decode errors and zero input drops; DABlin acquired DAB+ superframe sync, reported zero AU errors, and extracted 74,022 bytes of AAC data. Before TCP backpressure was added, the unpaced direct WAV command overran the input queue and disrupted superframes. ODR-DabMod is not installed in this environment; DABlin consumed the Rust mux's EDI output directly.
- Added TCP input backpressure: the receiver awaits space in its bounded channel, and the mux leaves queued frames in place when its playout buffer is full. UDP still drops old frames when necessary to stay live. Repeating the exact direct WAV encoder command for ten seconds produced zero input drops, zero AF decode errors and zero missed ticks. DABlin had one initial superframe acquisition, no subsequent sync restarts, zero AU errors and extracted 84,162 bytes of AAC. The aggregate underflow count includes all eleven unconnected services in the 12-service test config.
- Follow-up on intermittent live DABlin reacquisition: a 30-second EDI capture and a 35-second live TCP decode with the direct WAV command each had one initial sync, no later reacquisition and zero AU errors. A 25-second live DABlin run with SDL's dummy driver also stayed in sync. The mux now logs each subchannel's transition into and out of underflow, with actual and expected payload lengths, so a recurrence can be tied to its source rather than the aggregate underflow counter. The user's live connection command and stats at the failure are still needed to isolate the unreproduced condition.

### 2026-09-30 — bitrate hot-change verification

- A live API change from 72 to 64 kbit/s recalculated the subchannel from 54 to 48 CUs and from 216 to 192 payload bytes per frame. The changed input handle and its buffer are discarded; the old TCP encoder connection closes and a restarted 64 kbit/s encoder connects to the reused listener. The API returned `changed: true`, with zero input drops, AF decode errors and missed ticks in the local test.
- DABlin on an existing EDI connection saw the new 64 kbit/s FIG 0/1 entry but stopped extracting AAC after the change. This is receiver reacquisition, not stale mux input frames: DABlin's service decoder remains selected by the same subchannel ID. The mux does not yet announce a future ETSI reconfiguration point; its FIG 0/0 Change field remains zero. A newly started DABlin client decoded AAC at 64 kbit/s.
- Cleared TCP output preroll history on ensemble structure changes so new clients cannot receive pre-change frames before the new layout. Existing output clients remain connected to avoid breaking an EDI modulator connection. For now, bitrate changes require restarting or retuning receivers that do not reset themselves. Seamless on-air structural changes remain an open implementation gate requiring advance reconfiguration signalling and receiver interoperability testing.
- Follow-up with both encoders connected: the old 72 kbit/s ODR-AudioEnc automatically reconnected after the mux switched to 64 kbit/s, mixing 216-byte and 192-byte frames on the same input port. The TCP receiver now rejects wrong-size frames before buffering, closes that producer, and permits only one active matching producer. UDP also rejects wrong-size frames before buffering. `/api/stats` exposes `input_size_mismatches`, and the bitrate mismatch warning appears once per input lifetime instead of alternating underflow/recovery messages every frame. In a local overlap test with both encoders running, DABlin decoded the 64 kbit/s output with one superframe acquisition and zero AU errors; there were zero AF decode errors and input drops. The old encoder kept reconnecting, so it should still be stopped during a bitrate change.

### 2026-09-30 — review fixes

- Reviewed the complete Rust implementation against the C++ reference and EN 300 401, TS 102 693 and TS 101 756; findings and resolutions are in [REVIEW_FINDINGS.md](REVIEW_FINDINGS.md). The repository root moved from `dabmux/` to the workspace root with history rewritten under `dabmux/`. `Cargo.lock` is tracked.
- Vendored the EDInburgh FIC decoder as a test oracle. Its first run exposed swapped seconds/millisecond fields in FIG 0/10, which decoded 03:04:05.678 as 03:04:33.422. This is fixed, and the oracle now checks FIG 0/0, 0/1, 0/2, 0/5, 0/9, 0/10, 0/13 and FIG 1 over a 12-service carousel.
- Labels are UTF-8 in config and encoded as EBU Latin. The short-label mask and length limits apply to encoded characters, and unrepresentable characters are rejected.
- FIG 0/0 and 0/7 open each transmission frame instead of every CIF. FIG 0/5 signals each subchannel once, and conflicting languages on a shared subchannel are rejected. User applications are accepted only on a service's first component, since FIG 0/8 is not emitted.
- TCP inputs: the newest producer connection replaces the previous one, which supersedes the earlier "one active producer" rule. A producer silent for 10 s is disconnected. Wrong-size frames still close their producer before it can take over. Backpressure remains the TCP default; `backpressure: false` selects drop-oldest for live encoders. A trial with drop-oldest as the default broke the unpaced `odr-audioenc -i test.wav` workflow (729 drops, AU errors), so that default was reverted. Sustained overflow is logged per subchannel with a hint.
- Hot reload activates at a transmission-frame boundary. Only ensemble ID, sub-channel and service organisation count as structural, which clears TCP preroll and advances a configured FIG 0/7 counter. Inputs keep their connection and buffer unless their input settings or bitrate change. TCP output queue/preroll changes no longer disconnect clients. A bind failure on a port still held by the active config explains the two-step move.
- A failed frame is skipped and counted in `frame_errors` instead of stopping the mux. Failed clock changes during activation are returned to the caller. `/api/stats` adds `clock_drift_ms`.
- Verification: 49 unit tests (`cargo test`), strict Clippy and rustfmt pass. End to end, the real ODR-AudioEnc with the unpaced `test.wav` produced zero input drops, and DABlin decoded `Radio Zürich`/`Grüezi Mux`, correct UTC time and HE-AAC 72 kbit/s with one superframe sync. A live API update kept the encoder connection, and a label containing `~` was rejected.

**Still open:** advance reconfiguration signalling (FIG 0/0 change flags, next-configuration MCI), FIG 0/8, UEP, PFT FEC, multicast, and deployment-host verification of bulletins, real inputs and daylight-saving transitions. A debug build logged some missed ticks on a laptop; measure a release build under sustained load.

### 2026-10-01 — structured configuration

- Replaced the flat `services`/`subchannels`/`components` YAML, wired by uids, with the model in [specs/configuration.md](specs/configuration.md). Services own their components, and a component defines its own anonymous subchannel. Subchannels shared by several components live in a top-level `subchannels` map and are referenced by name. There is no compatibility path for the old format.
- `config/schema.rs` holds the operator format and normalizes it into the existing internal model (`Multiplex`: services, subchannels, components by index). The existing validation runs on that model, so the FIC writers, the frame loop and hot-reload input matching are unchanged. Subchannels and services are named in logs and errors by shared name or `<SId>/<position>`.
- SubChIds are optional: configured IDs are reserved first, then the lowest free ID is taken in first-use order. A reload that renumbers a kept subchannel logs one warning listing the changes. `GET /api/config/resolved` shows allocated IDs, CU layout and resolved defaults.
- Added `defaults.protection` (built-in EEP 3-A) and `defaults.edi` for EDI buffer, timing and backpressure settings. IDs, ECC, PTY, language and SubChIds accept `"0x4F32"`-style strings. Parse errors name the field via `serde_path_to_error`. Moved from the archived `serde_yaml` to `serde_norway`. `tai_clock_bulletins` is a YAML list instead of a pipe-separated string. Reload failures now log the full error chain instead of the outer context only.
- Converted both example configs and added `dabmux/config.mux-zh.example.yaml` (17 audio services, 822 CUs; SPI, the service ECC override and ZeroMQ output pending). Verification: 55 tests, strict Clippy and rustfmt pass. A live run of the mux-zh config showed the resolved view, field-path errors on `POST /api/config`, and two file reloads (dropping SubChIds, then inserting a service at the front) activating with the reallocation warning.

**Next:** SPI support on the new model: 32-bit data services, per-service ECC, packet subchannel and component fields, then the file input and enhanced-packet FEC.

### 2026-10-01 — SPI: data services in enhanced packet mode

- Downloaded the ETSI standards to `docs/etsi/pdf/` (gitignored), including EN 300 401 V2.2.1 (2026-06). Two V2.2.1 rules shaped this work: packet mode sub-channels shall apply the clause 5.3.5 FEC, so there is only `type: enhanced_packet`; and FIG 0/8 shall be signalled for every component with user applications, including slideshow in PAD, which the mux previously omitted.
- `dabmux::packet` (library): packet CRC and scanning, padding packets, the RS(204,188) encoder, the FEC frame (12 × 188 table, nine FEC packets at address 1022), the frame multiplexer and an independent syndrome-based stream verifier. Against ODR-DabMux v5.5.1 configured like `mux.conf`, the output is byte-identical over 24 501 frames of the full mux-zh `spi.bin`; a 401-frame slice is a test fixture ([devsupport/cpp-reference/](../devsupport/cpp-reference/README.md)).
- Configuration: data services (32-bit IDs, primary component in packet mode, no PTY or language), programme service `ecc`, packet component `packet_address`, `dscty` and `data_groups` (MOT and data groups by default for `spi`/`slideshow`), `user_applications: [spi]`, and the file input. Validation covers address range and uniqueness per sub-channel, input/type pairing, component limits (11 for 32-bit SIds) and the 25-byte FIG 0/9 extended field. A data service SId whose ECC differs from the ensemble's is logged as a warning, as for mux-zh's `0x44010001`.
- Runtime: the file input checks the whole file, repeats it, and adopts a changed file at its next wrap (at once when at a wrap); a damaged file keeps the previous content and a missing file pads. File addresses are compared with the configured components.
- FIC: new FIG 0/2 data-service and packet-component entries, 0/3, 0/8, 0/9 extended field, 0/13 with P/D = 1 and the SPI basic profile, 0/14 and 1/5. The carousel was rebuilt as a priority scheme: 0/0 and 0/7, then 0/10 and one label per frame, then one complete MCI pass per 96 ms, then all remaining space for 0/5, 0/8, 0/9, 0/13 and 0/17. For mux-zh with SPI a test confirms complete MCI in every 96 ms period and every information entry and label within 41 frames over 1 000 frames; the previous fixed-slot carousel needed about 1.3 s for FIG 0/13 alone at 17 services. The FIG bytes for the SPI reference match ODR-DabMux.
- Verification: 70 tests, strict Clippy and rustfmt pass. A release-build run of the mux-zh example produced EDI whose sub-channel 30 had 16 FEC frames with zero syndromes, no packet CRC or continuity errors and CRC-valid MOT data groups; DABlin decoded the programme signalling including FIG 0/8.

**Still open:** an SPI-capable receiver test, ZeroMQ output (out of scope unless required), packet input over the network, advance reconfiguration signalling, UEP, PFT FEC and multicast.

### 2026-10-01 — service following (FIG 0/6, 0/20, 0/21, 0/24)

- Downloaded TS 103 176 V2.6.1 and TR 101 496-2. Implemented service linking, OE services, frequency information and service component information against EN 300 401 V2.2.1 and TS 103 176. Linkage sets are configured on their key service; FI, foreign services and announced changes at the top level ([specs/configuration.md](specs/configuration.md)). ILS, P/D and the OE flag of DAB FI are derived; link order and field split follow TS 103 176 clause 5.2.4.1, including dead links, IdLP preference and data service key services.
- Frequencies are written in MHz and kept as whole kHz, so FM codes are exact (ODR-DabMux truncates 87.6 MHz to code 0). DRM above 32.767 MHz uses the mode E multiplier.
- FIC: the database definitions get a credited share of the FIC, 4 bytes per frame and up to a FIB, so large fields still fit. FIG 0/20 and change indications join the once-per-second rotation. Labels are paced to one cycle per 40 frames instead of one per frame, which freed about 500 bytes/s for mux-zh. With linking and OE services on all 17 mux-zh services, a test confirms every database entry within 10 s and all information within 41 frames.
- Hot reload derives CEIs for changed or removed database entries and short-form activation state for LA changes, sent for five seconds with the affected definitions withheld until then.
- Verification:
  - **C++ capture:** the fields of the converted example match ODR-DabMux v5.5.1 where it follows the standards; the departures are documented in [devsupport/cpp-reference/README.md](../devsupport/cpp-reference/README.md).
  - **Date-time:** the FIG 0/20 encoding reproduces TS 103 176 annex C.1.
  - **Live run:** a release-build run sent every field, cycling the whole database every 30 frames. A watched-file reload that deactivated a linkage set sent its short form for 4.95 s and then the LA = 0 definition.
  - **Suite:** 77 tests, strict Clippy and rustfmt pass.

**Still open:** a receiver that follows links, OE announcements (FIG 0/25, 0/26) and announcement support (FIG 0/18, 0/19).
