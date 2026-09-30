
# Rust DAB multiplexer

The Rust implementation is in `dabmux/`. It currently accepts EDI/DSTI over UDP or TCP and STI-D/RTP over UDP, then emits EDI over UDP or TCP at one frame every 24 ms. It supports programme audio services with EEP-A or EEP-B protection. See [the implementation plan](docs/IMPLEMENTATION_PLAN.md) for current validation evidence and remaining release gates.

Start with `cargo run --bin dabmux -- --config dabmux/config.example.yaml`. The example expects an EDI input on TCP port 9000; its output destination is defined in the YAML. HTTP stats are available at `/api/stats` on port 7777 by default. Use `--watch-config` for validated file reloads; `POST /api/config` accepts a complete JSON configuration. Services, components, subchannels, IDs, input bindings, output destinations and ensemble settings can all change live. The API reports success after the new configuration activates at the start of a transmission frame. If validation or binding a new endpoint fails, the active configuration continues. For a bitrate change, stop the old encoder and start one configured for the new bitrate; `/api/stats` counts wrong-size frames as `input_size_mismatches`. Existing receivers such as DABlin may need to reconnect because advance reconfiguration signalling is not yet implemented.

The existing C++ odr-dabmux is retained under `__ref/ODR-DabMux/` as a behavioral reference. It is not the active implementation.

For a field-by-field comparison with the supplied production multiplex, see [the production config gap audit](docs/PRODUCTION_CONFIG_GAP.md). Ensemble and service `short_label` fields are supported; each must select no more than eight characters, in order, from its full `label`.

The supplied 12-service INFO configuration has a validated Rust counterpart at [config.production.example.yaml](dabmux/config.production.example.yaml). It includes PTY, language, slideshow signalling, automatic local time offset, TAI bulletin URLs, a 2-second TIST lead, and the TCP queue, preroll and TAG alignment settings. Check the host timezone and HTTPS bulletin access before using it on air.

## Operating notes

- **Labels** are sent in the EBU Latin character set (TS 101 756 Annex C). UTF-8 text such as `Zürich` is converted. Characters outside the repertoire, such as `~`, `\` and non-Latin scripts, are rejected at validation. Label length and the short-label selection count characters, not UTF-8 bytes.
- **Input buffering.** Each input buffers up to `buffer_frames` frames. TCP inputs use backpressure by default: while the buffer is full the mux stops reading, which throttles the encoder. This is required for unpaced file encoders such as `odr-audioenc -i test.wav`, and the buffered depth stays at `buffer_frames` + 1. For a live encoder, `backpressure: false` makes a full buffer drop its oldest frame instead (`input_drops`), so latency stays bounded and clock drift is absorbed at the mux. UDP and STI inputs always drop. Any dropped frame breaks a DAB+ superframe, so the mux warns when an input keeps delivering faster than real time.
- **TCP input producers.** One producer feeds each TCP input. A new connection that sends a valid frame replaces the previous one, so a restarted encoder is never locked out by a dead connection. A producer that sends nothing for 10 s is disconnected.
- **Reconfiguration.** Changes activate at the start of a transmission frame (every 96 ms in mode I). A structural change (ensemble ID, sub-channel or service organisation) advances the FIG 0/7 reconfiguration counter when one is configured, unless the new configuration sets the counter itself. Label, PTY, language, input and output changes do not count as structural. Inputs keep their connection and buffer unless their input settings or bitrate change. TCP output clients stay connected when queue or preroll settings change.
- **Ports.** A port cannot move between an input and a TCP output, or between two input addresses, in a single update, because new sockets are bound before the old ones are released. Remove it in one update and reuse it in the next.
- **Timing.** After a scheduling stall, the mux skips the missed frames rather than sending a burst: DLFC jumps (`missed_ticks`) and TIST stays on schedule. The frame clock is set from system time at startup. `/api/stats` reports its difference from system time as `clock_drift_ms`, and a warning is logged above 100 ms, which requires a restart to realign. FIG 0/9 can only signal whole half hours of local time offset, so zones such as +5:45 are truncated to +5:30.
- **Errors.** A frame that cannot be built is skipped and counted in `frame_errors`; the mux keeps running.

A reference implementation of a rust project using configuration hot-reload is provided under `__ref/sonicecast/`. It is not the active implementation.

Documentation for ETSI standards relevant to the project can be found under `docs/etsi/README.md`.
