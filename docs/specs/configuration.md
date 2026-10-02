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
  - id: 0xe1401001
    label: SPI

    components:
      - type: enhanced_packet
        bitrate: 8
        input:
          protocol: file
          path: /var/lib/dabmux-rs/spi.bin
        packet_address: 1
        user_applications:
          - spi
```

`enhanced_packet` is packet mode with the Reed-Solomon FEC of EN 300 401 clause 5.3.5. Since V2.2.1 the standard requires that FEC for every packet mode sub-channel, so there is no plain packet type.

---

## Data services and packet mode

A service whose primary component is in packet mode is a data service. Its ID is the 32-bit form: ECC (8 bits), country (4 bits) and service reference (20 bits). The ECC is part of the ID, so data services take no `ecc`; the mux warns when it differs from the ensemble ECC. Data services have no `pty` or `language`, and carry no audio components.

Packet mode settings belong to the component, because one packet sub-channel can carry several components:

- `packet_address` (required): the address of this component's packets, 1 to 1023 except 1022, which the FEC packets use. The packets come ready-made from an encoder, so the address must match theirs; the mux warns when the file contains other addresses.
- `dscty`: the data service component type (TS 101 756 table 2b). It defaults to MOT (60) for `spi` and `slideshow` and is required otherwise.
- `data_groups`: whether MSC data groups are used. Defaults to `true`, as MOT requires.

`user_applications: [spi]` signals the SPI basic profile (TS 102 371). `spi` is accepted in packet mode only; `slideshow` works in both.

A programme service may also carry a packet mode component as a secondary component.

### File input

```yaml
input:
  protocol: file
  path: /var/lib/dabmux-rs/spi.bin
```

The file holds complete packets, as written by an SPI or MOT packet encoder. The mux reads it whole, checks every packet's length and CRC, and repeats it. A changed file takes over when the current one wraps, so no data group is cut; a damaged file keeps the previous content; a missing file sends padding packets after the current pass. A relative path is resolved against the working directory. Packet sub-channels take file inputs only, and file inputs feed packet sub-channels only.

### Other countries

A programme service from another country sets its ECC, which the mux signals in the FIG 0/9 extended field:

```yaml
  - id: 0x1498
    ecc: 0xe0
    label: BOLLERWAGEN
```

The configuration therefore describes the service in terms of its components; the top-level `subchannels` map exists only for subchannels shared between components.

---

## Service following

Service following tells receivers where else a service can be heard. It is configured per service where the service is the subject, and at the top level otherwise. [config.service-linking.example.yaml](../../dabmux/config.service-linking.example.yaml) shows every option.

```yaml
services:
  - id: 0x8daa
    label: Funk
    other_ensembles: [0x4ffe, 0x4ffd]   # FIG 0/24: also carried there
    linking:                            # FIG 0/6: this service is the key service
      - lsn: 0xabc
        hard: true                      # default true; false: related content
        active: true                    # default true
        links:
          - {type: dab, id: 0x8daf}
          - {type: fm, id: 0x1a2b}
          - {type: fm, id: 0x4c5d, ecc: 0x4f, preference: low}
          - {type: drm, id: 0xec1298}   # 24-bit DRM or AMSS identifier

other_services:                         # FIG 0/24 for services not carried here
  - {id: 0x8daf, ensembles: [0x4ffd]}

frequencies:                            # FIG 0/21, in MHz
  - type: dab
    eid: 0x4fff
    continuity: true
    frequencies:
      - {mhz: 234.208, adjacent: true, mode_i: true}
  - {type: fm, pi: 0x1234, frequencies: [87.6, 105.2]}
  - {type: drm, id: 0x12ab45, frequencies: [15.21]}
  - {type: amss, id: 0x33cc88, frequencies: [14.8]}

service_changes:                        # FIG 0/20
  - {id: 0x1234, change: addition, ascty: 63, at: 2026-11-01T13:00:00Z, label: Neu}
  - {id: 0xabcd, change: identity, transfer_sid: 0xef01}
```

**Derived values.** The mux derives the following from content rather than taking them as settings:
- **ILS flag:** a linkage set is international when it has DRM or AMSS links, or an identifier with another country's ECC. `international: true` forces it, and `false` is rejected when the content needs it.
- **Identifier order:** the key service first, then DAB services carried here, other DAB, RDS, then DRM/AMSS, with normal preference before low (TS 103 176 clause 5.2.3).
- **P/D flag:** in FIG 0/6, 0/20 and 0/24 it follows the service: a service carried here by its type, any other service by its ID width.
- **OE flag:** for DAB frequency information it is set unless `eid` is this ensemble. For FM, DRM and AMSS it is clear when the identifier belongs to a service here (its SId as PI code, or a linked ID). `other_ensemble` overrides it.

**Rules.**
- **LSN:** 1 to 0xFFF, and unique with the hard and international flags.
- **Active sets:** a service may have one active hard and one active soft linkage set.
- **Dead link:** a hard set without links stops service following to FM.
- **Data services:** they link to DAB (32-bit) and DRM identifiers only.
- **Continuity:** for FM, DRM and AMSS it is valid only with OE = 0.
- **Frequency steps:** DAB frequencies are multiples of 16 kHz, and FM frequencies 87.6 to 107.9 MHz in 100 kHz steps.
- **FIG 0/20:** `change` is `identity`, `addition`, `local_removal` or `global_removal`. A `label` is allowed only for a service not carried here, and it is then signalled with the change. Remove an entry once its change is complete.

**Changes.** A reload that changes or removes a linkage set, FI entry or OE entry sends its CEI for five seconds and withholds the new definition until then. A linkage set whose `active` alone changes gets the short-form activation state instead, deactivations first (TS 103 176 clauses 5.2.4 to 5.4.4).

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

  - id: 0xe1401001
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
        packet_address: 1
        user_applications:
          - spi

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
│       ├── user applications
│       └── packet address, DSCTy, data groups (packet mode)
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