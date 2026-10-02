# Rust DAB multiplexer

The Rust implementation is in `dabmux/`. It accepts EDI/DSTI over UDP or TCP and STI-D/RTP over UDP, and emits EDI over UDP or TCP at one frame every 24 ms. It supports programme audio services (DAB+ and MPEG audio) with EEP-A or EEP-B protection, and data services such as SPI in packet mode with the Reed-Solomon FEC of EN 300 401 clause 5.3.5, fed from packet files.

## Quick start

```shell
cargo run --bin dabmux -- --config dabmux/config.example.yaml
```

The example expects an EDI input on TCP port 9000 and serves EDI on TCP port 9001. HTTP stats are available at `/api/stats` on port 7777 by default. [devsupport/README.md](devsupport/README.md) has encoder and DABlin commands for an end-to-end check.

Use `--watch-config` for validated file reloads; `POST /api/config` accepts a complete JSON configuration. Services, components, subchannels, IDs, input bindings, output destinations and ensemble settings can all change live. The API reports success after the new configuration activates at the start of a transmission frame. If validation or binding a new endpoint fails, the active configuration continues. For a bitrate change, stop the old encoder and start one configured for the new bitrate; `/api/stats` counts wrong-size frames as `input_size_mismatches`. Existing receivers such as DABlin may need to reconnect after a structural change, because advance reconfiguration signalling is not yet implemented.

## Configuration

Services own their components, and each component normally defines its own subchannel; [docs/specs/configuration.md](docs/specs/configuration.md) describes the model.

```yaml
services:
  - id: 0x4f32
    label: Radio X
    components:
      - type: dab_plus
        subchannel_id: 1
        bitrate: 72
        input: {protocol: edi, uri: "tcp://:9000"}
        user_applications: [slideshow]
```

- **SubChIds.** `subchannel_id` is optional. Unset IDs take the lowest ID not configured elsewhere, in service and component order, so adding or removing a service can renumber the services after it. Receivers then lose those services until they rescan, and the mux logs a warning on reload. Set `subchannel_id` on air.
- **Shared subchannels.** A subchannel used by several components is defined once in the top-level `subchannels` map and referenced by name with `subchannel: <name>`. A reference carries no subchannel settings, and every named subchannel must be used.
- **Defaults.** Protection is EEP 3-A unless `defaults.protection` or the component sets it. `defaults.edi` sets `buffer_frames`, `prebuffer_frames`, `timing` and `backpressure` for EDI inputs that leave them unset; the `backpressure` default applies to TCP inputs only.
- **Numbers.** IDs, ECC, PTY and language accept integers or strings such as `"0x4F32"`, since JSON has no hexadecimal literals.
- **Data services and SPI.** A service whose primary component has `type: enhanced_packet` is a data service with a 32-bit ID. Its component reads ready-made packets with `input: {protocol: file, path: ...}` and sets `packet_address` to the address in those packets; `user_applications: [spi]` signals SPI and defaults the DSCTy to MOT. The file is checked packet by packet and repeated, and a changed file takes over when the current one wraps. A programme service from another country sets `ecc`, signalled in the FIG 0/9 extended field.
- **Service following.** A service's `linking` lists linkage sets with it as key service (FIG 0/6), and `other_ensembles` the ensembles that also carry it (FIG 0/24). Top-level `other_services`, `frequencies` (FIG 0/21, in MHz) and `service_changes` (FIG 0/20) complete it. Flags that EN 300 401 and TS 103 176 define from content are derived: ILS, P/D, and OE for DAB frequencies. A reload that changes these databases sends the change indications (CEI or activation state) for 5 s. [config.service-linking.example.yaml](dabmux/config.service-linking.example.yaml) covers every option.
- **Errors** name the field, for example `services[2] (4F2C).components[0] refers to unknown subchannel "x"`. `GET /api/config/resolved` shows the configuration as the mux runs it, with allocated SubChIds, CU addresses and defaults filled in.

The supplied 12-service INFO configuration has a validated Rust counterpart at [config.production.example.yaml](dabmux/config.production.example.yaml), and the mux-zh reference config, including its SPI service, at [config.mux-zh.example.yaml](dabmux/config.mux-zh.example.yaml). It includes PTY, language, slideshow signalling, automatic local time offset, TAI bulletin URLs, a 2-second TIST lead, and the TCP queue, preroll and TAG alignment settings. Check the host timezone and HTTPS bulletin access before using it on air.

## Documentation

- [Implementation plan](docs/IMPLEMENTATION_PLAN.md): architecture, phases, validation evidence and remaining release gates.
- [Production config gap audit](docs/PRODUCTION_CONFIG_GAP.md): field-by-field comparison with the supplied production multiplex.
- [Review findings](docs/REVIEW_FINDINGS.md): review against the C++ reference and ETSI standards, with resolutions.
- [Verification with etisnoop](docs/verification/etisnoop.md): capturing, converting to ETI and checking FIGs, rates and audio.
- [C++ reference research](docs/RESEARCH.md): how ODR-DabMux is structured and behaves.
- [ETSI standards index](docs/etsi/README.md).

## Operating notes

- **Labels** are sent in the EBU Latin character set (TS 101 756 Annex C). UTF-8 text such as `Zürich` is converted. Characters outside the repertoire, such as `~`, `\` and non-Latin scripts, are rejected at validation. Label length and the short-label selection count characters, not UTF-8 bytes. A `short_label` must select no more than eight characters, in order, from its full `label`.
- **Input buffering.** Each input buffers up to `buffer_frames` frames. TCP inputs use backpressure by default: while the buffer is full the mux stops reading, which throttles the encoder. This is required for unpaced file encoders such as `odr-audioenc -i test.wav`, and the buffered depth stays at `buffer_frames` + 1. For a live encoder, `backpressure: false` makes a full buffer drop its oldest frame instead (`input_drops`), so latency stays bounded and clock drift is absorbed at the mux. UDP and STI inputs always drop. Any dropped frame breaks a DAB+ superframe, so the mux warns when an input keeps delivering faster than real time.
- **TCP input producers.** One producer feeds each TCP input. A new connection that sends a valid frame replaces the previous one, so a restarted encoder is never locked out by a dead connection. A producer that sends nothing for 10 s is disconnected.
- **Reconfiguration.** Changes activate at the start of a transmission frame (every 96 ms in mode I). A structural change (ensemble ID, sub-channel or service organisation) advances the FIG 0/7 reconfiguration counter when one is configured, unless the new configuration sets the counter itself. Label, PTY, language, input and output changes do not count as structural. Inputs keep their connection and buffer unless their input settings or bitrate change. TCP output clients stay connected when queue or preroll settings change.
- **Ports.** A port cannot move between an input and a TCP output, or between two input addresses, in a single update, because new sockets are bound before the old ones are released. Remove it in one update and reuse it in the next.
- **Timing.** After a scheduling stall, the mux sends the late frames back to back, each with its scheduled DLFC and TIST, as ODR-DabMux does; a TIST modulator still plays them. `/api/stats` counts them as `catch_up_frames`. More than 10 s behind, the frames are too late for any TIST offset and the burst would overflow the TCP output queues, so the mux skips ahead on the 24 ms grid instead: DLFC jumps, `missed_ticks` counts the skipped frames and a warning is logged. The frame clock is set from system time at startup. `/api/stats` reports its difference from system time as `clock_drift_ms`, and a warning is logged above 100 ms, which requires a restart to realign. FIG 0/9 can only signal whole half hours of local time offset, so zones such as +5:45 are truncated to +5:30.
- **Errors.** A frame that cannot be built is skipped and counted in `frame_errors`; the mux keeps running.
- **FIC.** Each 96 ms period carries the complete MCI (FIG 0/1, 0/2, 0/3, 0/14). Labels are paced to one full cycle per second. FIG 0/5, 0/8, 0/9, 0/13, 0/17 and 0/20 fill the remaining space and repeat at least once per second for the 18-service mux-zh configuration. The service following databases get a steady share of about 170 bytes/s, enough for linkage sets and OE services on all 17 mux-zh services within 10 s. Much larger ensembles can exceed the FIC capacity.

## Development

```shell
cargo test
make -C dabmux fmt clippy
```

The FIG writers are tested against an independent FIC decoder vendored from EDInburgh under `dabmux/src/testsupport/edinburgh/`, which is compiled only for tests, and against FIGs and packet-mode output captured from the C++ ODR-DabMux ([devsupport/cpp-reference/](devsupport/cpp-reference/README.md)). ETSI standards are downloaded to `docs/etsi/pdf/` (see [.claude/rules/etsi-standards.md](.claude/rules/etsi-standards.md)).

## References

`__ref/` holds local reference code and is not tracked in git:

- `__ref/ODR-DabMux/`: the C++ ODR-DabMux (GPL-3.0-or-later), the behavioural reference for this port. It is not the active implementation.
- `__ref/edinburgh/`: the EDInburgh Rust EDI receiver (GPL-2.0). Its EBU Latin table (`dabmux/src/charset.rs`) and FIC decoder (test only) are reused; confirm the licence terms before distribution.
- `__ref/sonicecast/`: a Rust project using configuration hot reload, used as a pattern reference.
