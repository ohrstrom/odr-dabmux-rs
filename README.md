# ODR-DabMux-RS

A DAB/DAB+ multiplexer written in Rust, based on [ODR-DabMux](https://github.com/Opendigitalradio/ODR-DabMux).

ODR-DabMux-RS combines audio and data services into a DAB ensemble and outputs an EDI stream for transmission.

## Features

- **DAB/DAB+ audio** — MPEG audio and DAB+ services with EEP-A/B protection.
- **Data services** — Packet mode, SPI and Reed-Solomon FEC.
- **Network inputs** — EDI/DSTI over UDP or TCP, STI-D/RTP over UDP.
- **EDI output** — UDP and TCP, with 24 ms frame timing.
- **Live configuration** — Update services, subchannels and outputs without restarting.
- **HTTP API** — Configuration management and runtime statistics.
- **Service information** — Labels, programme types, service linking and ensemble information.

## Quick start

Run the multiplexer using the included example configuration:

```bash
cargo run --bin dabmux -- --config dabmux/config.example.yaml
```

The example configuration uses:

| Interface  | Address         |
| ---------- | --------------- |
| EDI input  | TCP port `9000` |
| EDI output | TCP port `9001` |
| HTTP API   | Port `7777`     |

Web interface: `http://localhost:7777`

Runtime statistics: `http://localhost:7777/api/stats`.

For an end-to-end test with an encoder and DABlin, see [Development support](devsupport/README.md).

## Configuration

The multiplexer uses YAML configuration files.

Example DAB+ service:

```yaml
services:
  - id: 0x4f32
    label: Radio X
    components:
      - type: dab_plus
        subchannel_id: 1
        bitrate: 72
        input:
          protocol: edi
          uri: "tcp://:9000"
        user_applications: [slideshow]
```

EEP 3-A is the default protection level. Subchannel IDs can be assigned automatically, but explicit IDs are recommended for production use.

Additional examples:

- [Basic configuration](dabmux/config.example.yaml)
- [Production configuration](dabmux/config.production.example.yaml)
- [SPI and data services](dabmux/config.mux-zh.example.yaml)
- [Service linking](dabmux/config.service-linking.example.yaml)

See the [configuration reference](docs/specs/configuration.md) for all available options.

## Live configuration and API

Configuration can be updated while the multiplexer is running.

Start with automatic configuration reload:

```bash
cargo run --bin dabmux -- \
  --config dabmux/config.example.yaml \
  --watch-config
```

The HTTP API provides:

| Endpoint                   | Description                         |
| -------------------------- | ----------------------------------- |
| `GET /api/stats`           | Runtime statistics                  |
| `GET /api/config/resolved` | Active, resolved configuration      |
| `GET /api/ui/config`       | Configuration and pending changes   |
| `POST /api/config`         | Apply a complete JSON configuration |

Changes to the multiplex structure are announced to receivers before activation, following ETSI EN 300 401. Invalid configurations leave the running multiplex unchanged.

## Development

Run the tests:

```bash
cargo test
```

Check formatting and run Clippy:

```bash
make -C dabmux fmt clippy
```

### Documentation

- [Configuration reference](docs/specs/configuration.md)
- [Implementation and architecture](docs/IMPLEMENTATION_PLAN.md)
- [ETI/EDI verification](docs/verification/etisnoop.md)
- [ETSI standards](docs/etsi/README.md)
- [C++ reference and comparison](docs/RESEARCH.md)

## Credits

This project is a Rust port of [ODR-DabMux](https://github.com/Opendigitalradio/ODR-DabMux), developed by the Opendigitalradio contributors.

Thanks to the original developers and the open-source digital radio community.

## License

Licensed under the **GNU General Public License v3.0 or later (GPL-3.0-or-later)**.

See [LICENSE](LICENSE) and [NOTICE.md](NOTICE.md) for details.
