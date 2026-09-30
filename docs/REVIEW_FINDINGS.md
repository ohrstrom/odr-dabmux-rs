# Rust implementation review findings

Review date: 2026-09-30. Scope: all of `dabmux/src/` (about 3.8k lines), checked against the C++ reference in `__ref/ODR-DabMux/` and against EN 300 401, TS 102 693 and TS 101 756 from memory. The PDFs are not in the repository, so spec references are to clause content, not page numbers. `cargo test` passes (32 tests) with this code as reviewed.

Severity: **High** = wrong on-air data or loss of service, **Medium** = incorrect in realistic use, **Low** = deviation, robustness or cleanup.

## Summary

| # | Severity | Area | Finding |
|---|----------|------|---------|
| 1 | High | FIG 0/10 | **Fixed.** Seconds and millisecond MSBs were swapped in the long-form UTC byte |
| 2 | High | TCP EDI input | A half-open producer connection can block the subchannel indefinitely |
| 3 | Medium | Labels | **Fixed.** ASCII was sent unconverted as EBU Latin. `$ \ ^ \` { \| } ~` display as other glyphs, and umlauts are rejected |
| 4 | Medium | TCP EDI input | Backpressure pushes clock drift back to the encoder, so nothing bounds latency or absorbs drift |
| 5 | Medium | Reconfiguration | The FIG 0/7 counter is not incremented on structural changes, and the change is not aligned to a CIF-count boundary |
| 6 | Low | FIC | FIG 0/0 (and FIG 0/7) are sent in every CIF; C++ sends them only at frame phase 0 |
| 7 | Low | FIG 0/13 | Secondary components get SCIdS > 0 without FIG 0/8 |
| 8 | Low | FIG 0/5 | Shared subchannels produce duplicate or conflicting language entries |
| 9 | Low | Hot reload | Restarts sever encoder and receiver TCP sessions more often than needed |
| 10 | Low | Hot reload | Moving a port from an input to a TCP output fails with EADDRINUSE |
| 11 | Low | Robustness | Any `?` error in the frame loop stops the whole mux |
| 12 | Low | Timing | Local time offset truncates 45-minute zones. The clock is never re-synced to UTC |
| 13 | Low | Buffering | Effective input buffering is about 2 × `buffer_frames` |
| 14 | Low | Cleanup | URI parsing is duplicated between `config.rs` and `runtime.rs` |

## Details

### 1. FIG 0/10 long-form UTC field layout (High) — fixed

**Status:** fixed in `fic.rs`. `oracle_tests::decoder_reads_fig0_10_date_and_time` covers it; before the fix it decoded 03:04:05.678 as 03:04:33.422.

[fic.rs:301](../dabmux/src/fic.rs#L301):

```rust
((second & 0x3f) | (((millis >> 8) & 3) << 6)) as u8,
```

Per EN 300 401 §8.1.3.1, the long-form UTC is hours(5), minutes(6), seconds(6), milliseconds(10). After the minutes byte, the next byte is **seconds in bits 7..2** followed by **ms bits 9..8 in bits 1..0**. The C++ struct `FIGtype0_10_LongForm` (`Milliseconds_high:2; Seconds:6;`, GCC allocates bitfields LSB-first) confirms this. The Rust code has the two fields reversed, so receivers decode wrong seconds and milliseconds. Hours, minutes and MJD are correct.

The unit test only checks time 0, where every field is zero, so it cannot catch this.

Fix: `(((second & 0x3f) << 2) | ((millis >> 8) & 3)) as u8`. Add a test with non-zero seconds and milliseconds, e.g. 03:04:05.678.

The edinburgh receiver's decoder (`__ref/edinburgh/shared/src/dab/fic.rs`, `Fig0_10::from_bytes`) reads `second = data[4] >> 2`. This independently confirms the correct layout, and a round-trip test through that decoder would have caught the bug (see R2 below).

### 2. TCP EDI input: a stale connection locks out reconnects (High)

[runtime.rs:222-296](../dabmux/src/runtime.rs#L222-L296): the first connection that delivers a valid frame becomes `active_client`. Any other connection is closed on its first frame ("ignoring additional TCP EDI producer"). The ownership is released only when the owner's `read_exact` fails. There is no read timeout and no TCP keepalive. If the encoder host reboots or the network path drops without a FIN or RST, the old socket stays half-open and blocks forever. The reconnecting encoder is then rejected indefinitely, and the subchannel outputs silence until the mux restarts or its config changes.

C++ (`TCPReceiveServer::process`, `lib/Socket.cpp:1253`) disconnects after 10 s without data.

Candidates:
- Wrap reads in `tokio::time::timeout` (e.g. 2–10 s) and release ownership on timeout.
- Or let the newest producer take over, since an encoder reconnecting is the common case.
- Enable `SO_KEEPALIVE` as an additional safeguard.

### 3. Label character set (Medium) — fixed

**Status:** fixed. Labels and short labels are encoded with `dabmux::charset` (R1). Validation rejects characters outside the repertoire and measures length and the short-label mask on the encoded bytes.

[config.rs:584-593](../dabmux/src/config.rs#L584-L593) accepts printable ASCII only, and [fic.rs:312](../dabmux/src/fic.rs#L312) copies the bytes verbatim into FIG 1 with charset 0 (EBU Latin, TS 101 756 Annex C). EBU Latin differs from ASCII at 0x24 `$`→`ł`, 0x5C `\`→`Ů`, 0x5E `^`→`Ł`, 0x60 `` ` ``→`Ą`, 0x7B `{`→`«`, 0x7C `|`→`ů`, 0x7D `}`→`»`, 0x7E `~`→`Ľ` (see `lib/charset/charset.cpp`). Labels using these characters display differently on receivers.

UTF-8 input such as `ä ö ü ß` is rejected, although EBU Latin can encode it and C++ converts it. This matters for a German-language multiplex.

Candidate: invert the edinburgh `EBU_LATIN_TO_UNICODE` table (see R1 below) to get a UTF-8 → EBU Latin encoder. Validate the length after conversion (16 bytes), and compute the short-label mask on the converted bytes.

### 4. TCP input backpressure versus clock drift (Medium)

For TCP inputs, `backpressure: true` ([runtime.rs:101](../dabmux/src/runtime.rs#L101), [:106](../dabmux/src/runtime.rs#L106)) stops draining the channel once `queue.len() == max_frames`, and the receiver task then blocks on `tx.send().await`. With prebuffering, frames are therefore never dropped. An encoder whose capture clock runs slightly fast accumulates latency until the channel, queue and TCP socket buffers are full. After that, the encoder blocks on send, which typically causes an ALSA overrun or dropped audio in ODR-AudioEnc.

C++ prebuffering drops frames when the buffer exceeds its maximum. This keeps latency bounded and absorbs drift at the mux. The current behaviour suits file-fed test encoders that run faster than real time, but not live capture.

Candidate: make backpressure opt-in (e.g. `input.backpressure: true` for file sources). By default, drop the oldest frames when the queue exceeds `buffer_frames`, as UDP inputs already do.

### 5. Reconfiguration signalling (Medium)

- When `reconfiguration_counter` is configured, FIG 0/7 carries it verbatim ([fic.rs:46-52](../dabmux/src/fic.rs#L46-L52)). A structural change detected at [runtime.rs:753-767](../dabmux/src/runtime.rs#L753-L767) does not increment it, so a receiver that relies on FIG 0/7 sees no change. EN 300 401 requires the count to change with each reconfiguration. Candidate: auto-increment modulo 1024 when `structure_changed`, unless the new config explicitly changes the counter.
- The switch happens on whichever frame follows preparation. EN 300 401 §6.5 reconfigures at a signalled CIF count, and in mode I that is at a transmission-frame boundary (CIF count mod 4 == 0). Aligning the switch to `clock.count % 4 == 0` is cheap and a step towards the full advance signalling already listed in the README. The full signalling uses the FIG 0/0 change flags, the next-configuration FIG 0/1 and 0/2, and the occurrence change.

### 6. FIG 0/0 cadence (Low)

[fic.rs:41-52](../dabmux/src/fic.rs#L41-L52) writes FIG 0/0 (and FIG 0/7) into FIB 0 of every CIF. C++ sends them only when `framephase == 0` (`src/fig/FIGCarousel.cpp:300-440`), i.e. once per 96 ms mode I transmission frame, as the first FIG of the frame. Receivers tolerate the higher rate, but it costs 6–10 bytes per CIF. Some decoders use FIG 0/0 at phase 0 as a transmission-frame marker. Candidate: emit it only on `clock.count % 4 == 0` for modes I, II and IV. Check the mode III rule before changing that mode.

### 7. FIG 0/13 SCIdS without FIG 0/8 (Low)

[fic.rs:216-224](../dabmux/src/fic.rs#L216-L224) derives SCIdS from the component's position within its service. For the primary component this is 0, which is correct. A slideshow on a secondary component would get SCIdS ≥ 1, but no FIG 0/8 maps SCIdS to a subchannel, so receivers cannot resolve it. Candidate: either restrict user applications to the primary component in validation, or emit FIG 0/8.

### 8. FIG 0/5 with shared subchannels (Low)

Language entries are built per component and keyed by SubChId ([fic.rs:170-187](../dabmux/src/fic.rs#L170-L187)). Validation allows two services to share one subchannel (the tests do this). The mux then emits duplicate entries, or conflicting ones if the two services have different languages. Candidate: deduplicate by SubChId, or reject conflicting languages.

### 9. Hot reload restarts more sessions than necessary (Low)

- [runtime.rs:612-627](../dabmux/src/runtime.rs#L612-L627): an input handle is kept only if its whole `SubchannelConfig` is unchanged. Changing the protection level, subchannel ID or uid aborts the receiver task. That drops the encoder's TCP connection and the buffered frames, and forces prebuffering again. Only a changed input or bitrate actually requires this.
- [runtime.rs:641-667](../dabmux/src/runtime.rs#L641-L667): changing `max_frames_queued` or `preroll_ms` on a TCP output aborts the server task. The `JoinSet` of client tasks is dropped with it, so every connected modulator is disconnected.

### 10. Port moved from input to output (Low)

`prepare_resources` binds new listeners while the old ones are still open. A candidate that frees port N from an input and reuses it as a TCP output (or the other way round) fails with EADDRINUSE and is rejected. This is safe, but it is surprising. Document the behaviour or handle this case.

### 11. Any `?` error stops the mux (Low)

Every `?` inside the frame loop in `runtime::run` ([runtime.rs:770-918](../dabmux/src/runtime.rs#L770-L918)) ends the runtime, and `try_join!` then stops the process. Examples are the carousel, `local_offset_half_hours`, `assemble`, and the seconds conversion. Most of these cannot fail with a validated config. Two can: a timezone lookup failure, and TAI arithmetic after an unexpected bulletin value. Candidate: log the error, keep the last good FIC or frame, count the failure in stats, and continue.

### 12. Timing details (Low)

- `local_offset_half_hours` ([timing.rs:96-103](../dabmux/src/timing.rs#L96-L103)) truncates, so Nepal (+5:45) becomes +5:30. FIG 0/9 can only express half hours anyway, so this only needs documenting.
- The frame clock is derived from wall time once at startup and afterwards advanced only by `tick()`. It follows the tokio monotonic clock and ignores NTP steps. C++ behaves the same way. For SFN use, consider comparing it against `SystemTime` periodically and warning (or rephasing) when the difference exceeds a threshold.
- After a stall, `MissedTickBehavior::Skip` plus explicit `clock.tick()` calls skip frames: DLFC jumps and TIST stays on schedule. This is a reasonable choice, but it differs from C++ (which bursts) and should be documented.

### 13. Buffering depth (Low)

The mpsc channel has capacity `buffer_frames`, and `BufferedInput.queue` holds up to `buffer_frames` more ([runtime.rs:538](../dabmux/src/runtime.rs#L538)). For UDP inputs the effective worst-case depth is therefore about twice the configured value. Candidate: use a small channel capacity, or document the combined depth.

### 14. Cleanup (Low)

- URI to socket-address parsing is implemented twice: [config.rs:377-410](../dabmux/src/config.rs#L377-L410) and `bind_address`/`input_key` in [runtime.rs:192-209](../dabmux/src/runtime.rs#L192-L209), [:461-468](../dabmux/src/runtime.rs#L461-L468). Resolve it once during validation, e.g. store `SocketAddr` and transport in `ValidatedSubchannel`.
- `fic.rs` repeatedly looks up services and subchannels by uid with linear searches on every frame. Precompute the lookups in `ValidatedConfig`.

## Reuse candidates from `__ref/edinburgh`

`__ref/edinburgh` is a Rust EDI receiver and player. Its `shared` crate decodes the same structures the mux encodes. It is licensed GPLv2; its LICENSE file is the plain GPLv2 text, and the individual sources state neither "or later" nor "only". Before copying code into this project, confirm the licence terms with the edinburgh author (possibly the same people). They matter because this project is a port of ODR-DabMux, which is GPLv3-or-later. GPLv2-only code cannot be combined with it, while GPLv2-or-later code can.

### R1. EBU Latin table (recommended, fixes finding 3) — done

**Status:** implemented as `dabmux/src/charset.rs`.

`shared/src/dab/tables.rs`: `EBU_LATIN_TO_UNICODE: [u16; 256]`. I compared it entry by entry against the C++ `utf8_encoded_EBU_Latin`, taking into account that the C++ table starts at index 1. **255 of 256 entries match.** The only difference is 0x1F, which TS 101 756 defines as a control code (preferred word break): edinburgh maps it to U+001F, C++ maps it to U+0082. Labels should never contain control codes, so for encoding this doesn't matter. The table has no duplicate code points, so it inverts cleanly into a `char → u8` encoder.

Suggested use: a `charset` module with `encode_ebu_latin(&str) -> Result<Vec<u8>>` built from the inverted table. Reject 0x00–0x1F and 0x7F. Use it in `validate_label` / `short_label_mask` and `fig1_label`. Keep the decode direction too, for tests.

### R2. FIC decoder as a test oracle (recommended) — done

**Status:** vendored under `dabmux/src/testsupport/edinburgh/` (test builds only; `__ref/` is gitignored, so a path dependency would break fresh clones). The tests are in `fic.rs` `oracle_tests`: a 12-service carousel run checks FIG 0/0, 0/1 (EEP-A and EEP-B), 0/2, 0/5, 0/9, 0/13 and FIG 1 labels with umlauts, and a separate test checks FIG 0/10. The tests verify FIB CRCs themselves.

`shared/src/dab/fic.rs` (`FicDecoder::from_bytes`) decodes FIB CRC, FIG 0/0, 0/1, 0/2, 0/3, 0/5, 0/9, 0/10, 0/13 and FIG 1/0, 1/1 (label plus short-label mask). Its bit layouts agree with EN 300 401 and C++ for everything the mux emits. Running `FicCarousel` output through it in unit tests would verify every field the mux emits against a decoder written by someone else. Finding 1 is exactly the kind of bug that shows up in such a test.

To use it: add a `[dev-dependencies] shared = { path = "../__ref/edinburgh/shared" }` (it pulls in tokio, serde, md5, base64 and others, which is acceptable for tests only). Alternatively, vendor `fic.rs`, `tables.rs` and `utils.rs` into `dabmux/tests/support/`.

Limitations to keep in mind when using it as an oracle:
- It has no FIG 0/7 or 0/17 decoder, so those still need hand-written assertions.
- FIG 0/1 skips SubChId > 30 ("Ignore sc_id > 30"), whereas the valid range is 0–63. Tests must keep IDs ≤ 30 or patch this.
- FIG 0/9 returns `lto` as integer hours (`half_hours / 2`), which loses half-hour offsets. Assert on the raw bytes for that.
- Decoding errors of individual FIGs are silently dropped inside `decode_fib`. Tests should assert that the expected FIGs are present, not just that decoding succeeds.

### R3. EDI frame/tag parser as a round-trip oracle (optional)

`shared/src/dab/frame.rs` parses AF tags, `deti` (flags, MID, FIC length and ATST/RFUD length consistency) and `est`. It checks less than the mux's own `AfPacket::decode`. It is useful mainly because it is a second, independent reading of the DETI header layout, and it feeds the FIC decoder above. Low effort to add alongside R2.

### R4. DAB+ superframe sync / Fire code (optional, input health)

`shared/src/dab/msc.rs` (`re_sync`) and `utils::calc_crc_fire_code` detect DAB+ superframe boundaries (a Fire code over bytes 2..11 of each 5-frame superframe). The mux passes subchannel data through without looking at it. Running this check on inputs would give a cheap "input is a valid DAB+ stream" health metric in `/api/stats`. It would also detect misaligned or wrong-bitrate encoders earlier than the size check does. This is not needed for correctness.

### R5. Language and user-application tables (optional)

`tables.rs` has a `Language` enum (TS 101 756 Table 9/10 codes, 0x08 = German) and `UserApplication` type constants (SLS 0x002, SPI 0x007, Journaline 0x44A, …). They could let the YAML accept `language: deu` or `German` instead of `0x08`, and could replace the magic `0x42` in the FIG 0/13 writer. They are cosmetic, but they make the config clearer.

Not worth reusing: `utils::calc_crc16_ccitt` duplicates `edi::crc16`, which is already correct. The MOT, DL and PAD decoders concern the encoder side, not the multiplexer.

## Checked and found consistent

These match the spec or C++ and need no change:

- EEP-A and EEP-B sizes and ETI/EDI TPL encoding
- ETI frame layout: FSYNC alternation, FC, STC, EOH/MNSC and CRCs, MST CRC, EOF, TIST, FL bound, 6144-byte limit
- FIG 0/1 long form, FIG 0/2 (ASCTy 63 for DAB+, P/S flag), FIG 0/9 LTO sign bit, FIG 0/17 (matches C++ static PTy layout), FIG 1 label with character-flag mask, FIB end marker and padding, FIB CRC
- FIG 0/13 slideshow data (X-PAD AppTy 12, DSCTy 60), matching C++
- AF header (`0x90 'T'`) and CRC, TAG length in bits, DETI header and ETI header bit positions, ATST (UTCO = TAI−UTC−32, seconds since 2000 with UTCO added, matching C++ `set_edi_time`), EST SSTC
- DSTI flag handling and `ss` tag header skip. The PFT header without FEC and the fragment count are also correct
- `tagpacket_alignment: 16` appends a fixed 8-byte `*dmy`, like C++ `TagPacket.cpp:65`. Neither implementation guarantees 16-byte alignment, so Rust is consistent with C++ here
- STI-D/RTP parsing matches C++ `Sti_d_Rtp::receive_packet`. Rust also handles `CRCSTF` correctly, where C++ has an operator-precedence bug (`& 0x80 >> 7`)
- Startup grid alignment and `tist_at_fct0` phasing match C++ `MuxTime::init`
