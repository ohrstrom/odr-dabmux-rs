
# Rust DAB multiplexer

The Rust implementation is in `dabmux/`. It currently accepts EDI/DSTI over UDP or TCP and STI-D/RTP over UDP, then emits EDI over UDP or TCP at one frame every 24 ms. It supports programme audio services with EEP-A or EEP-B protection. See [the implementation plan](docs/IMPLEMENTATION_PLAN.md) for current validation evidence and remaining release gates.

Start with `cargo run --bin dabmux -- --config dabmux/config.example.yaml`. The example expects an EDI input on TCP port 9000; its output destination is defined in the YAML. HTTP stats are available at `/api/stats` on port 7777 by default. Use `--watch-config` for validated file reloads; `POST /api/config` accepts a complete JSON configuration. Services, components, subchannels, IDs, input bindings, output destinations and ensemble settings can all change live. The API reports success after the new configuration activates on a frame boundary. If validation or binding a new endpoint fails, the active configuration continues. For a bitrate change, stop the old encoder and start one configured for the new bitrate; `/api/stats` counts wrong-size frames as `input_size_mismatches`. Existing receivers such as DABlin may need to reconnect because advance reconfiguration signalling is not yet implemented.

The existing C++ odr-dabmux is retained under `__ref/ODR-DabMux/` as a behavioral reference. It is not the active implementation.

For a field-by-field comparison with the supplied production multiplex, see [the production config gap audit](docs/PRODUCTION_CONFIG_GAP.md). Ensemble and service `short_label` fields are supported; each must select no more than eight characters, in order, from its full `label`.

The supplied 12-service INFO configuration has a validated Rust counterpart at [config.production.example.yaml](dabmux/config.production.example.yaml). It includes PTY, language, slideshow signalling, automatic local time offset, TAI bulletin URLs, a 2-second TIST lead, and the TCP queue, preroll and TAG alignment settings. Check the host timezone and HTTPS bulletin access before using it on air.

A reference implementation of a rust project using configuration hot-reload is provided under `__ref/sonicecast/`. It is not the active implementation.

Documentation for ETSI standards relevant to the project can be found under `docs/etsi/README.md`.
