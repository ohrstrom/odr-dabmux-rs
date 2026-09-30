# Rust DAB multiplexer

The Rust implementation is in `dabmux/`. It accepts EDI/DSTI over UDP or TCP and STI-D/RTP over UDP, and emits EDI over UDP or TCP at one frame every 24 ms. It supports programme audio services (DAB+ and MPEG audio) with EEP-A or EEP-B protection.

## Quick start

```shell
cargo run --bin dabmux -- --config dabmux/config.example.yaml
```

The example expects an EDI input on TCP port 9000 and serves EDI on TCP port 9001. HTTP stats are available at `/api/stats` on port 7777 by default. [devsupport/README.md](devsupport/README.md) has encoder and DABlin commands for an end-to-end check.

Use `--watch-config` for validated file reloads; `POST /api/config` accepts a complete JSON configuration. Services, components, subchannels, IDs, input bindings, output destinations and ensemble settings can all change live. The API reports success after the new configuration activates at the start of a transmission frame. If validation or binding a new endpoint fails, the active configuration continues. For a bitrate change, stop the old encoder and start one configured for the new bitrate; `/api/stats` counts wrong-size frames as `input_size_mismatches`. Existing receivers such as DABlin may need to reconnect after a structural change, because advance reconfiguration signalling is not yet implemented.

The supplied 12-service INFO configuration has a validated Rust counterpart at [config.production.example.yaml](dabmux/config.production.example.yaml). It includes PTY, language, slideshow signalling, automatic local time offset, TAI bulletin URLs, a 2-second TIST lead, and the TCP queue, preroll and TAG alignment settings. Check the host timezone and HTTPS bulletin access before using it on air.

## Documentation

- [Implementation plan](docs/IMPLEMENTATION_PLAN.md): architecture, phases, validation evidence and remaining release gates.
- [Production config gap audit](docs/PRODUCTION_CONFIG_GAP.md): field-by-field comparison with the supplied production multiplex.
- [Review findings](docs/REVIEW_FINDINGS.md): review against the C++ reference and ETSI standards, with resolutions.
- [C++ reference research](docs/RESEARCH.md): how ODR-DabMux is structured and behaves.
- [ETSI standards index](docs/etsi/README.md).

## Operating notes

- **Labels** are sent in the EBU Latin character set (TS 101 756 Annex C). UTF-8 text such as `Zürich` is converted. Characters outside the repertoire, such as `~`, `\` and non-Latin scripts, are rejected at validation. Label length and the short-label selection count characters, not UTF-8 bytes. A `short_label` must select no more than eight characters, in order, from its full `label`.
- **Input buffering.** Each input buffers up to `buffer_frames` frames. TCP inputs use backpressure by default: while the buffer is full the mux stops reading, which throttles the encoder. This is required for unpaced file encoders such as `odr-audioenc -i test.wav`, and the buffered depth stays at `buffer_frames` + 1. For a live encoder, `backpressure: false` makes a full buffer drop its oldest frame instead (`input_drops`), so latency stays bounded and clock drift is absorbed at the mux. UDP and STI inputs always drop. Any dropped frame breaks a DAB+ superframe, so the mux warns when an input keeps delivering faster than real time.
- **TCP input producers.** One producer feeds each TCP input. A new connection that sends a valid frame replaces the previous one, so a restarted encoder is never locked out by a dead connection. A producer that sends nothing for 10 s is disconnected.
- **Reconfiguration.** Changes activate at the start of a transmission frame (every 96 ms in mode I). A structural change (ensemble ID, sub-channel or service organisation) advances the FIG 0/7 reconfiguration counter when one is configured, unless the new configuration sets the counter itself. Label, PTY, language, input and output changes do not count as structural. Inputs keep their connection and buffer unless their input settings or bitrate change. TCP output clients stay connected when queue or preroll settings change.
- **Ports.** A port cannot move between an input and a TCP output, or between two input addresses, in a single update, because new sockets are bound before the old ones are released. Remove it in one update and reuse it in the next.
- **Timing.** After a scheduling stall, the mux skips the missed frames rather than sending a burst: DLFC jumps (`missed_ticks`) and TIST stays on schedule. The frame clock is set from system time at startup. `/api/stats` reports its difference from system time as `clock_drift_ms`, and a warning is logged above 100 ms, which requires a restart to realign. FIG 0/9 can only signal whole half hours of local time offset, so zones such as +5:45 are truncated to +5:30.
- **Errors.** A frame that cannot be built is skipped and counted in `frame_errors`; the mux keeps running.

## Development

```shell
cargo test
make -C dabmux fmt clippy
```

The FIG writers are tested against an independent FIC decoder vendored from EDInburgh under `dabmux/src/testsupport/edinburgh/`, which is compiled only for tests.

## References

`__ref/` holds local reference code and is not tracked in git:

- `__ref/ODR-DabMux/`: the C++ ODR-DabMux (GPL-3.0-or-later), the behavioural reference for this port. It is not the active implementation.
- `__ref/edinburgh/`: the EDInburgh Rust EDI receiver (GPL-2.0). Its EBU Latin table (`dabmux/src/charset.rs`) and FIC decoder (test only) are reused; confirm the licence terms before distribution.
- `__ref/sonicecast/`: a Rust project using configuration hot reload, used as a pattern reference.
