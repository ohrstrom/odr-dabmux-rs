# Production multiplex configuration gap audit

Compared on 2026-09-30 against the supplied `he2-mux` INFO configuration, `__ref/ODR-DabMux/doc/advanced.mux`, the C++ parser/FIG writers, and the Rust code. A validated conversion is available as [`dabmux/config.production.example.yaml`](../dabmux/config.production.example.yaml); the mux-zh reference config is covered [below](#mux-zh-reference-configuration). Rust intentionally has a smaller input/output and operations scope.

## Production fields

| Production setting | Rust status | Detail |
| --- | --- | --- |
| `dabmode 1`, `nbframes 0`, `throttle "simul://"` | Implemented | Mode I and a continuous 24 ms real-time frame loop. There is no frame-count or throttle option. |
| Ensemble `id`, `ecc`, `international-table`, `label` | Implemented | YAML `ensemble.id`, `ecc`, `international_table`, `label`; FIG 0/0, 0/9 and FIG 1. |
| Ensemble `shortlabel "RND D00"` | Implemented | YAML `ensemble.short_label`. FIG 1 carries a character-selection mask, independently decoded by DABlin. Omission selects the first eight positions, matching C++ default. |
| Twelve service IDs, full labels and short labels | Implemented | YAML `services[]` with `id`, `label` and `short_label`; FIG 0/2 and FIG 1. Labels are UTF-8 in YAML and sent as EBU Latin, like C++. The supplied 12-service graph fits the current audio model. |
| Service `pty 15` | Implemented | YAML `services[].pty`; static FIG 0/17. DABlin decoded `Other Music` with international table 1. |
| Service `language 0x08` | Implemented | YAML `services[].language`; FIG 0/5. DABlin decoded German on all 12 subchannels. |
| Twelve DAB+ EDI/TCP inputs, EEP-A level 3, subchannel IDs and bitrates | Implemented | Each service's component: `type: dab_plus`, `subchannel_id`, `bitrate`, `input: {protocol: edi, uri: tcp://...}`; EEP 3-A is the default protection (`defaults.protection` or per-component `protection`). The supplied 4×72 + 4×64 + 4×48 kbit/s arrangement uses 552 of 864 CUs. TCP inputs throttle their encoder by default (`backpressure`); a newer producer connection replaces the previous one, and a producer silent for 10 s is disconnected. |
| Service-to-subchannel components | Implemented | YAML `services[].components[]`; each component defines its own subchannel, so no service/component/subchannel uids are wired by hand. FIG 0/2 identifies the audio components. |
| `user-applications { userapp "slideshow" }` on every component | Implemented | YAML `services[].components[].user_applications: [slideshow]`; FIG 0/13 signals application type `0x2`, X-PAD application type `12` and MOT DSCTy `60`. DABlin decoded Slideshow on the 12-service test stream. FIG 0/8, which EN 300 401 clause 6.3.5 requires for components with user applications, is emitted for each of them, so secondary components can carry applications too. |
| `tist true` | Implemented | TIST and EDI timestamps use either a fixed TAI–UTC offset or configured bulletin URLs. |
| `tist_offset 2` | Implemented | YAML `ensemble.tist_offset_ms: 2000`; a live DETI check measured approximately 1.986 seconds of lead, and a live update to zero changed it to approximately −0.013 seconds. |
| `tai_clock_bulletins ...` | Implemented | YAML `ensemble.tai_clock_bulletins` lists HTTPS URLs, tried in order at startup and refreshed hourly. A valid expired bulletin is a last resort, with a warning. There is no persistent cache across process restarts. |
| `local-time-offset auto` | Implemented | YAML `ensemble.local_time_offset_auto: true` reads the host timezone for FIG 0/9, including daylight-saving changes. The test stream decoded as +02:00 in the current host timezone. |
| EDI TCP listener on port 8850, `enable_pft false` | Implemented | YAML TCP EDI output sends AF packets. PFT/FEC is not used on TCP. |
| `max_frames_queued 500` | Implemented | YAML TCP destination `max_frames_queued: 500`; each client has an exact bounded queue and is dropped when it fills. |
| `preroll-burst 3.5` | Implemented | YAML TCP destination `preroll_ms: 3500`; the last 146 frames are queued to each new client before live frames. A local client received 120 frames in roughly 2 ms. |
| `tagpacket_alignment 16` | Implemented | YAML `output.tagpacket_alignment: 16`; each TAG packet appends a `*dmy` item with eight bytes of data, matching the C++ setting. |
| `fec 2`, EDI `port 5000` | No effect for this output | The production destination is TCP with PFT disabled. FEC and fallback UDP port do not affect that active path. |
| `managementport`, remote telnet/ZMQ | Replaced by design | Rust exposes stats through HTTP and accepts complete configuration updates through file reload or API push. It does not implement C++ management protocols. |
| `startupcheck`, `syslog false` | Missing/different | Rust does not gate startup on `chronyc waitsync`; logging uses tracing rather than the C++ syslog switch. `/api/stats` reports `clock_drift_ms` between the frame clock and system time, and a warning is logged above 100 ms. |
| Reconfiguration counter | Not set | The production config sets none, so FIG 0/7 is not sent. If `ensemble.reconfiguration_counter` is set, structural hot reloads advance it automatically. |

The supplied layout totals **736 kbit/s**, **552 CUs**, **12 EST payloads** and **2208 MSC bytes per frame**. The FIG carousel completes the 12-service PTY/language/slideshow metadata cycle within 48 frames (1.152 seconds). DABlin independently decoded the full 12-service signalling from the live Rust stream. The converted YAML validates, but its bulletin URLs and real input feeds have not been exercised on the deployment host.

## mux-zh reference configuration

[`docs/reference-configs/mux-zh/mux.conf`](reference-configs/mux-zh/mux.conf) has 17 DAB+ services and an SPI data service. It converts to [`dabmux/config.mux-zh.example.yaml`](../dabmux/config.mux-zh.example.yaml), which validates and runs: 828 of 864 CUs, with `buffer 100` / `prebuffering 30` expressed once as `defaults.edi`.

| Setting | Rust status | Detail |
| --- | --- | --- |
| `srv-spi`, `id 0x44010001` | Implemented | Data service with a 32-bit ID: FIG 0/2 with P/D = 1, FIG 1/5 label, FIG 0/8 and FIG 0/13 with the SPI basic profile. |
| `type enhancedpacket`, `bitrate 8` | Implemented | `type: enhanced_packet`: packet mode with RS(204,188) FEC, nine FEC packets at address 1022, FIG 0/14 scheme 1. |
| `inputproto file`, `load_entire_file true` | Implemented | `input: {protocol: file, path}`: the file is read whole, checked packet by packet, repeated, and replaced at its next wrap when it changes, like C++. A missing file sends padding packets. |
| Component `type 60`, `address 0x1`, `datagroup true`, `userapp "spi"` | Implemented | `packet_address: 1` and `user_applications: [spi]`; DSCTy 60 and data groups are the defaults for SPI. FIG 0/3 signals them. |
| Service `ecc 0xE0` (BOLLERWAGEN, 0x1498) | Implemented | `ecc: 0xe0`, signalled in the FIG 0/9 extended field. |
| `outputs.zeromq` on `tcp://*:8950` | Out of scope by design | Needed only if a consumer of port 8950 cannot move to EDI. |

**Verification.** Against ODR-DabMux v5.5.1 configured like `mux.conf` ([devsupport/cpp-reference/](../devsupport/cpp-reference/README.md)):
- **Packet stream:** the Rust packet multiplexer reproduced the C++ sub-channel 30 output byte for byte over 24 501 frames of the full `spi.bin`, including the wrap, and all 237 FEC frames passed an independent syndrome check.
- **FIGs:** the FIG 0/2, 0/3, 0/5, 0/8, 0/9, 0/13, 0/14, 1/0, 1/1 and 1/5 bytes match the C++ ones.
- **Live run:** the Rust mux ran the mux-zh example. In 40 s of its TCP EDI output, sub-channel 30 had 16 FEC frames with zero syndromes, no packet CRC or continuity errors, and MSC data groups with valid CRCs. DABlin decoded the programme signalling, including the new FIG 0/8.

No SPI receiver has displayed the guide yet.

**Two values to check with the operator.** The C++ mux passes both through unchanged:
- `0x44010001` decodes as ECC 0x44 and country 0 rather than E1/4; the Rust mux logs a warning for it.
- `srv-rockantenne` (`0x121B`, a German ID) has no `ecc 0xE0`.

## service-linking reference configuration

[`docs/reference-configs/misc/service-linking.conf`](reference-configs/misc/service-linking.conf) is the ODR-DabMux example for service following. Its Rust counterpart is [`dabmux/config.service-linking.example.yaml`](../dabmux/config.service-linking.example.yaml).

| Section | Rust status | Detail |
| --- | --- | --- |
| `linking { set-… }` | Implemented | `services[].linking`, with the service as key service; FIG 0/6. The ILS flag is derived (DRM/AMSS links or a foreign ECC), links follow the TS 103 176 transmission order and Step A/B split, `preference: low` sets IdLP, and an empty hard set is a dead link. Data service key services (P/D = 1) are supported. |
| `other-services { … }` | Implemented | `services[].other_ensembles` (OE = 0) and top-level `other_services` (OE = 1); FIG 0/24, P/D from the service. |
| `frequency_information { … }` | Implemented | Top-level `frequencies`, in MHz and stored as whole kHz; FIG 0/21 for DAB, FM, DRM (including mode E) and AMSS. OE is derived for DAB and, for other bearers, from whether the id belongs to a service here; `other_ensemble` overrides it. |
| `service-component-information { … }` | Implemented | Top-level `service_changes`; FIG 0/20 with change type, part-time flag, component type, `at` date-time, transfer SId/EId, and an optional label for services not yet in the ensemble. |
| CEI and activation changes | Implemented | Derived on reload: a changed or removed entry gets a CEI and an LA change gets the short form, each for 5 s, deactivations first. The C++ mux needs an explicit empty linkage set instead. |

The fields match ODR-DabMux v5.5.1 for this config except where C++ departs from the standards; [devsupport/cpp-reference/README.md](../devsupport/cpp-reference/README.md) lists the differences. A live run of the Rust mux sent every field and, on reload, the LA = 0 short form for 4.95 s. No receiver has followed a link yet.

## Short-label semantics

`advanced.mux` and `src/MuxElements.cpp` define the short label as a selection of at most eight characters from the 16-character FIG 1 full label. It is not a second freely encoded text string. For `105 DJ HRND-001` → `HRND-001`, the mask is `0x01fe`; for `RND D00 - XX` → `RND D00`, it is `0xfe00`. Rust validates this relationship for ensemble and service labels, emits the mask in FIG 1, and accepts live changes through the full-config reload path. Labels are encoded in the EBU Latin character set (TS 101 756 Annex C), as in C++; the mask and the 16/8 character limits apply to the encoded characters. Characters outside the repertoire are rejected at validation.

## Additional advanced.mux capabilities outside this production example

The reference also documents announcements, linking, frequency information, FIG 2/extended and component labels, stream-mode data, UEP, multicast input/output, PFT/FEC and packet spreading. They are not exposed by the current Rust model. The selected project scope still excludes non-EDI/STI inputs, non-EDI outputs and the C++ control protocols.

## Deployment checks still needed

1. Verify the two HTTPS bulletin URLs and clock synchronization on the deployment host. The Rust process has no persistent bulletin cache or `startupcheck` command.
2. Feed the real 12 EDI inputs and verify audio and slideshow payload reception by the target modulator/receivers; this test used zero-filled MSC underflow data.
3. Test a daylight-saving transition and long-running TAI refresh on the deployment host.
4. Structural hot reloads are announced as EN 300 401 clause 6.5 describes: FIG 0/0 Change flags 11 with the occurrence change, and the next configuration's MCI (FIG 0/1, 0/2, 0/3, 0/7, 0/8, 0/13, 0/14) with C/N = 1, for 5.5 to 6 s before the switch at the announced CIF. Configurations stay at least 6 s. Still to do: test with receivers that implement reconfiguration (DABlin is not a reliable reference here) and with ODR-DabMod, whose handling of capacity decreases (the n − 15 rule, clause 6.5 and annex D) is unverified. A bitrate change still interrupts that service's audio until the restarted encoder delivers frames at the new size.
5. The production inputs rely on the TCP backpressure default. For live encoders whose clock should be absorbed at the mux, set `backpressure: false` per input and watch `input_drops` and the "faster than real time" warning.
