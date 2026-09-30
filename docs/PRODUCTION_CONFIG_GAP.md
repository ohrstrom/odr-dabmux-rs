# Production multiplex configuration gap audit

Compared on 2026-09-30 against the supplied `he2-mux` INFO configuration, `__ref/ODR-DabMux/doc/advanced.mux`, the C++ parser/FIG writers, and the Rust code. A validated conversion is available as [`dabmux/config.production.example.yaml`](../dabmux/config.production.example.yaml). Rust intentionally has a smaller input/output and operations scope.

## Production fields

| Production setting | Rust status | Detail |
| --- | --- | --- |
| `dabmode 1`, `nbframes 0`, `throttle "simul://"` | Implemented | Mode I and a continuous 24 ms real-time frame loop. There is no frame-count or throttle option. |
| Ensemble `id`, `ecc`, `international-table`, `label` | Implemented | YAML `ensemble.id`, `ecc`, `international_table`, `label`; FIG 0/0, 0/9 and FIG 1. |
| Ensemble `shortlabel "RND D00"` | Implemented | YAML `ensemble.short_label`. FIG 1 carries a character-selection mask, independently decoded by DABlin. Omission selects the first eight positions, matching C++ default. |
| Twelve service IDs, full labels and short labels | Implemented | YAML `services[]` and `short_label`; FIG 0/2 and FIG 1. The supplied 12-service graph fits the current audio model. |
| Service `pty 15` | Implemented | YAML `services[].pty`; static FIG 0/17. DABlin decoded `Other Music` with international table 1. |
| Service `language 0x08` | Implemented | YAML `services[].language`; FIG 0/5. DABlin decoded German on all 12 subchannels. |
| Twelve DAB+ EDI/TCP inputs, EEP-A level 3, subchannel IDs and bitrates | Implemented | YAML `kind: dab_plus`, `protection: {profile: eep_a, level: 3}`, `input: {protocol: edi, uri: tcp://...}`. The supplied 4×72 + 4×64 + 4×48 kbit/s arrangement uses 552 of 864 CUs. |
| Service-to-subchannel components | Implemented | YAML `components[]`; FIG 0/2 identifies the audio components. |
| `user-applications { userapp "slideshow" }` on every component | Implemented | YAML `components[].user_applications: [slideshow]`; FIG 0/13 signals application type `0x2`, X-PAD application type `12` and MOT DSCTy `60`. DABlin decoded Slideshow on the 12-service test stream. |
| `tist true` | Implemented | TIST and EDI timestamps use either a fixed TAI–UTC offset or configured bulletin URLs. |
| `tist_offset 2` | Implemented | YAML `ensemble.tist_offset_ms: 2000`; a live DETI check measured approximately 1.986 seconds of lead, and a live update to zero changed it to approximately −0.013 seconds. |
| `tai_clock_bulletins ...` | Implemented | Pipe-separated HTTPS URLs are tried in order at startup and refreshed hourly. A valid expired bulletin is a last resort, with a warning. There is no persistent cache across process restarts. |
| `local-time-offset auto` | Implemented | YAML `ensemble.local_time_offset_auto: true` reads the host timezone for FIG 0/9, including daylight-saving changes. The test stream decoded as +02:00 in the current host timezone. |
| EDI TCP listener on port 8850, `enable_pft false` | Implemented | YAML TCP EDI output sends AF packets. PFT/FEC is not used on TCP. |
| `max_frames_queued 500` | Implemented | YAML TCP destination `max_frames_queued: 500`; each client has an exact bounded queue and is dropped when it fills. |
| `preroll-burst 3.5` | Implemented | YAML TCP destination `preroll_ms: 3500`; the last 146 frames are queued to each new client before live frames. A local client received 120 frames in roughly 2 ms. |
| `tagpacket_alignment 16` | Implemented | YAML `output.tagpacket_alignment: 16`; each TAG packet appends a `*dmy` item with eight bytes of data, matching the C++ setting. |
| `fec 2`, EDI `port 5000` | No effect for this output | The production destination is TCP with PFT disabled. FEC and fallback UDP port do not affect that active path. |
| `managementport`, remote telnet/ZMQ | Replaced by design | Rust exposes stats through HTTP and accepts complete configuration updates through file reload or API push. It does not implement C++ management protocols. |
| `startupcheck`, `syslog false` | Missing/different | Rust does not gate startup on `chronyc waitsync`; logging uses tracing rather than the C++ syslog switch. |

The supplied layout totals **736 kbit/s**, **552 CUs**, **12 EST payloads** and **2208 MSC bytes per frame**. The FIG carousel completes the 12-service PTY/language/slideshow metadata cycle within 48 frames (1.152 seconds). DABlin independently decoded the full 12-service signalling from the live Rust stream. The converted YAML validates, but its bulletin URLs and real input feeds have not been exercised on the deployment host.

## Short-label semantics

`advanced.mux` and `src/MuxElements.cpp` define the short label as a selection of at most eight characters from the 16-character FIG 1 full label. It is not a second freely encoded text string. For `105 DJ HRND-001` → `HRND-001`, the mask is `0x01fe`; for `RND D00 - XX` → `RND D00`, it is `0xfe00`. Rust now validates this relationship for ensemble and service labels, emits the mask in FIG 1, and accepts live changes through the full-config reload path. The Rust ASCII-only label restriction remains narrower than C++ EBU Latin support.

## Additional advanced.mux capabilities outside this production example

The reference also documents announcements, linking, frequency information, FIG 2/extended and component labels, multiple user applications, packet/data services, UEP, multicast input/output, PFT/FEC and packet spreading. They are not exposed by the current Rust model. The selected project scope still excludes non-EDI/STI inputs, non-EDI outputs and the C++ control protocols.

## Deployment checks still needed

1. Verify the two HTTPS bulletin URLs and clock synchronization on the deployment host. The Rust process has no persistent bulletin cache or `startupcheck` command.
2. Feed the real 12 EDI inputs and verify audio and slideshow payload reception by the target modulator/receivers; this test used zero-filled MSC underflow data.
3. Test a daylight-saving transition and long-running TAI refresh on the deployment host.
4. Structural hot reload is not yet seamless for receivers. A local 72→64 kbit/s change was reflected in FIG 0/1 and MSC payload size, but DABlin on the existing connection stopped decoding audio until it was restarted. TCP preroll history now clears on structural changes; advance ETSI reconfiguration signalling and receiver interoperability testing remain required for seamless on-air changes.
