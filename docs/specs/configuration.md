# dabmux-rs Multiplexer Configuration Model

## Goal

The dabmux-rs multiplexer configuration should expose the DAB service/component model without requiring operators to manually construct and connect separate `service`, `component`, and `subchannel` objects.

The common case should be concise:

```yaml
services:
  - id: 0x4f32
    label: Radio X

    components:
      - type: dab_plus
        bitrate: 72
        input:
          protocol: edi
          uri: tcp://:9000
```

Internally this still represents:

```text
Service
└── Service Component
    └── Subchannel
        └── Input
```

The component and subchannel remain separate concepts in the internal DAB model. The configuration merely makes the normal one-component/one-subchannel relationship implicit.

This avoids synthetic configuration identifiers such as:

```yaml
uid: srv-radiox
uid: comp-radiox
uid: sub-radiox
```

and avoids manually wiring those identifiers together.

---

## Services and components

A service contains one or more components:

```yaml
services:
  - id: 0x4f32
    label: Radio X
    short_label: Radio X
    pty: 15
    language: 0x0f

    components:
      - type: dab_plus
        bitrate: 72
        protection:
          profile: eep_a
          level: 3
        input:
          protocol: edi
          uri: tcp://:9000
        user_applications:
          - slideshow
```

For this common form dabmux-rs creates an anonymous subchannel automatically.

Conceptually:

```text
Service "Radio X"
└── Component 0
    └── automatically allocated subchannel
        ├── type: DAB+
        ├── bitrate: 72 kbit/s
        ├── protection: EEP-A level 3
        └── input: EDI over TCP
```

The numeric DAB SubChId can also be allocated automatically.

---

## Multiple components

A service may contain multiple components:

```yaml
services:
  - id: 0x4f32
    label: Radio X

    components:
      - type: dab_plus
        bitrate: 72
        input:
          protocol: edi
          uri: tcp://:9000

      - type: dab_plus
        bitrate: 48
        input:
          protocol: sti
          uri: rtp://:9001
```

Each component creates its own subchannel unless it explicitly references a shared subchannel.

Component ordering determines the component ordering within the service, including the primary component and subsequent SCIdS values.

---

## Component types

The component `type` describes the kind of subchannel required by the component.

For DAB+ audio:

```yaml
components:
  - type: dab_plus
    bitrate: 72
    input:
      protocol: edi
      uri: tcp://:9000
```

For MPEG audio:

```yaml
components:
  - type: mpeg_audio
    bitrate: 128
    input:
      protocol: edi
      uri: tcp://:9000
```

For an enhanced packet component, for example SPI:

```yaml
services:
  - id: 0x44010001
    label: SPI

    components:
      - type: enhanced_packet
        bitrate: 8
        input:
          protocol: file
          path: /var/lib/dabmux-rs/spi.bin
```

Packet mode, the file input and the data service fields (32-bit service IDs, packet address, DSCTy, data groups) are not implemented yet; this example shows the intended shape.

The configuration therefore describes the service in terms of its components; the top-level `subchannels` map exists only for subchannels shared between components.

---

## Input protocol and transport

The input protocol remains explicit because protocol and transport are separate concepts.

For EDI over TCP:

```yaml
input:
  protocol: edi
  uri: tcp://:9000
```

For EDI over UDP:

```yaml
input:
  protocol: edi
  uri: udp://:9000
```

For STI over RTP:

```yaml
input:
  protocol: sti
  uri: rtp://:9000
```

Protocol-specific options can be added to the same object:

```yaml
input:
  protocol: edi
  uri: tcp://:9000
  stream_index: 1
  buffer_frames: 40
  prebuffer_frames: 4
  timing: prebuffering
  backpressure: false
```

This maps naturally to a tagged Rust/Serde enum and allows each protocol to expose only the settings relevant to it.

---

## Shared subchannels

Normally every component gets its own anonymous subchannel.

For the less common case where several components use the same subchannel, the subchannel is defined once in the top-level `subchannels` map under a configuration-local name, and each component references it:

```yaml
subchannels:
  shared_audio:
    type: dab_plus
    bitrate: 72
    protection:
      profile: eep_a
      level: 3
    input:
      protocol: edi
      uri: tcp://:9000

services:
  - id: 0x4001
    label: Service A
    components:
      - subchannel: shared_audio

  - id: 0x4002
    label: Service B
    components:
      - subchannel: shared_audio
```

Conceptually:

```text
Service A
└── Component 0 ──┐
                  │
                  ├── Subchannel "shared_audio"
                  │       ├── DAB+
                  │       ├── 72 kbit/s
                  │       └── EDI/TCP input
                  │
Service B         │
└── Component 0 ──┘
```

`shared_audio` is only a configuration-local symbolic name. It is not the transmitted numeric DAB SubChId; a shared subchannel sets that with `id`.

Keeping the definition outside the services means removing Service A cannot break Service B, and every user of the subchannel reads the same way. The rules are strict:

- A component either defines its own subchannel or references a shared one. A reference carrying `type`, `bitrate`, `protection`, `input` or `subchannel_id` is rejected.
- A reference must name an entry in `subchannels`.
- Every entry in `subchannels` must be used by at least one component.

---

## Explicit DAB SubChId

Normally dabmux-rs allocates the numeric DAB SubChId automatically.

Where a stable or explicitly controlled SubChId is required, it can be specified independently from the symbolic subchannel name: `subchannel_id` on a component that defines its own subchannel, `id` on a shared subchannel.

```yaml
subchannels:
  shared_audio:
    id: 7
    type: dab_plus
    bitrate: 72
    input:
      protocol: edi
      uri: tcp://:9000

services:
  - id: 0x4f32
    label: Radio X
    components:
      - subchannel_id: 1
        type: dab_plus
        bitrate: 72
        input:
          protocol: edi
          uri: tcp://:9001
```

The distinction is:

```text
subchannel: shared_audio
    Configuration-local symbolic name/reference.

subchannel_id: 1  (component)  /  id: 7  (shared subchannel)
    Actual DAB SubChId transmitted in the ensemble.
```

Both are optional in the normal case.

Automatic allocation is deterministic and depends on the configuration only. Configured IDs are reserved first; every other subchannel takes the lowest free ID, with subchannels ordered by first use in service and component order. The same order fixes their CU start addresses. Inserting or removing a service therefore renumbers the automatic IDs after it, and receivers lose those services until they rescan. A reload that renumbers kept subchannels logs a warning. On-air configurations should set SubChIds explicitly.

`GET /api/config/resolved` shows the allocated IDs, CU addresses and resolved defaults.

Subchannels are named in logs and errors by their shared name, or `<SId>/<component position>` (for example `4F32/0`) when anonymous.

---

## User applications

User applications belong to the service component:

```yaml
components:
  - type: dab_plus
    bitrate: 72
    input:
      protocol: edi
      uri: tcp://:9000
    user_applications:
      - slideshow
```

This preserves the distinction between the component and its underlying subchannel even though the subchannel itself is normally implicit in the configuration.

---

## Protection

Protection remains a property of the implicitly created subchannel:

```yaml
components:
  - type: dab_plus
    bitrate: 72
    protection:
      profile: eep_a
      level: 3
    input:
      protocol: edi
      uri: tcp://:9000
```

Protection defaults to EEP-A level 3. A top-level `defaults` block overrides the built-in defaults for protection and EDI input settings, so a deployment states its standard once:

```yaml
defaults:
  protection:
    profile: eep_a
    level: 3
  edi:
    buffer_frames: 100
    prebuffer_frames: 30
    backpressure: false   # TCP inputs only
```

The common configuration then reduces to:

```yaml
components:
  - type: dab_plus
    bitrate: 72
    input:
      protocol: edi
      uri: tcp://:9000
```

Settings on a subchannel or input always take precedence over `defaults`.

---

## Complete example

A typical ensemble configuration could therefore look approximately like:

```yaml
ensemble:
  id: 0x4401
  ecc: 0xe1
  label: DIG D04 - ZH
  short_label: DIG D04

services:
  - id: 0x4f32
    label: Radio X
    short_label: Radio X
    pty: 15
    language: 0x0f

    components:
      - type: dab_plus
        bitrate: 72
        protection:
          profile: eep_a
          level: 3
        input:
          protocol: edi
          uri: tcp://:9000
        user_applications:
          - slideshow

  - id: 0x4002
    label: Stadtfilter
    short_label: Stadtfil
    pty: 15
    language: 0x08

    components:
      - type: dab_plus
        bitrate: 64
        protection:
          profile: eep_a
          level: 3
        input:
          protocol: edi
          uri: tcp://:9001
        user_applications:
          - slideshow

  - id: 0x44010001
    label: SPI
    short_label: SPI

    components:
      - type: enhanced_packet
        bitrate: 8
        protection:
          profile: eep_a
          level: 3
        input:
          protocol: file
          path: /var/lib/dabmux-rs/spi.bin

output:
  tagpacket_alignment: 16
  destinations:
    - protocol: tcp
      listen_port: 8850
      max_frames_queued: 500
      preroll_ms: 3500
```

---

## Numbers

IDs, ECC, PTY, language and SubChIds accept an integer or a string. In YAML, `0x4f32` is an integer but `0X4F32` is read as a string, and JSON has no hexadecimal literals at all. So `"0x4F32"`, `0X4F32` and `20274` are equivalent everywhere, including `POST /api/config`.

---

## Configuration model

The desired user-facing model is therefore:

```text
Config
├── Ensemble
│
├── Defaults
│
├── Subchannels{name}   (shared only)
│
├── Service[]
│   ├── service properties
│   │
│   └── Component[]
│       ├── type
│       ├── optional reference to a shared subchannel
│       ├── optional explicit DAB SubChId
│       ├── bitrate
│       ├── protection
│       ├── input
│       └── user applications
│
└── Output
```

dabmux-rs normalizes this into the full internal DAB model:

```text
Config
        │
        ▼
normalization / validation
        │
        ▼
Service[]
Component[]
Subchannel[]
        │
        ▼
validated DAB multiplex
```

The important principle is that **subchannels remain first-class objects internally but are implicit in the normal user-facing configuration**.

The configuration should require explicit subchannel identity only when that identity actually matters: sharing a subchannel between components or controlling the transmitted numeric SubChId.

This gives the common case a small configuration surface while retaining the graph structure needed for more advanced DAB configurations.