# ODR-DabMux reference captures

Fixtures that compare the Rust mux with the C++ ODR-DabMux (v5.5.1, built in `__ref/ODR-DabMux/`) byte for byte.

## Enhanced packet (SPI)

[spi.mux](spi.mux) uses the SPI sub-channel of [docs/reference-configs/mux-zh/mux.conf](../../docs/reference-configs/mux-zh/mux.conf) (enhanced packet, 8 kbit/s, SubChId 30, address 1, `userapp "spi"`) and the BOLLERWAGEN service with its `ecc 0xE0`, writing 401 raw ETI frames.

```shell
cd "$(mktemp -d)"
head -c 7200 /path/to/odr-dabmux-rs/docs/reference-configs/mux-zh/spi.bin > spi-head.bin   # 300 packets
head -c 108000 /dev/zero > audio.dabp
/path/to/odr-dabmux-rs/__ref/ODR-DabMux/odr-dabmux /path/to/odr-dabmux-rs/devsupport/cpp-reference/spi.mux
/path/to/odr-dabmux-rs/devsupport/cpp-reference/eti_extract.py out.eti 30 subch30.bin
```

`spi-head.bin` and `subch30.bin` are `dabmux/tests/fixtures/packet/spi-head.bin` and `spi-head.odr-dabmux-subch30.bin`. The printed FIGs are the reference for the FIG writer tests.

For a longer capture, raise `nbframes`, use the full `spi.bin` and run the ignored comparison test:

```shell
PACKET_INPUT=spi.bin PACKET_REFERENCE=subch30.bin cargo test --lib capture_from_environment -- --ignored --nocapture
```

On 2026-10-01 the full mux-zh `spi.bin` over 24 501 frames (one complete pass plus the wrap) matched byte for byte, and all 237 FEC frames passed the syndrome check.

## Service following (FIG 0/6, 0/20, 0/21, 0/24)

[service-linking.mux](service-linking.mux) is [docs/reference-configs/misc/service-linking.conf](../../docs/reference-configs/misc/service-linking.conf) with file inputs and raw ETI output; [dabmux/config.service-linking.example.yaml](../../dabmux/config.service-linking.example.yaml) is its Rust counterpart.

```shell
cd "$(mktemp -d)"
head -c 28800 /dev/zero > audio.dabp
/path/to/odr-dabmux-rs/__ref/ODR-DabMux/odr-dabmux /path/to/odr-dabmux-rs/devsupport/cpp-reference/service-linking.mux
/path/to/odr-dabmux-rs/devsupport/cpp-reference/eti_extract.py link.eti 1 /dev/null
```

`fic::tests::service_following_matches_odr_dabmux_where_it_follows_the_standard` checks the Rust fields against this capture. Where ODR-DabMux v5.5.1 departs from EN 300 401 V2.2.1 and TS 103 176 V2.6.1, the test asserts the standard's encoding instead:

- **Linkage set order:** C++ splits linkage sets by link type in config order, and gives AMSS its own field. TS 103 176 clause 5.2.4.1 orders DAB, RDS, then DRM/AMSS, which share IdLQ 11.
- **`hard soft`:** C++ silently reads it as hard.
- **FM frequencies:** C++ truncates them in float arithmetic, so 87.6 and 105.2 MHz become codes 0 and 176 instead of 1 and 177.
- **C/N flags:** C++ sends new FIG 0/21 and 0/24 database keys with C/N = 1 when they follow other entries; each key's first field needs C/N = 0.

## FIC scheduling

The Rust mux schedules the FIC differently from all three ODR-DabMux v5.5.1 schedulers. The FIG bytes are the same; what differs is how often each FIG goes out.

### ODR-DabMux

`fic-scheduler` in the general section selects one of three schedulers. All of them use the same per-FIG rates (`src/fig/FIG.h`): rate A is 240 ms, not 96 ms, and B is 960 ms. A source comment says the rates were relaxed because full DAB+ ensembles cannot meet the recommended ones. FIG 0/14 is rate B, and FIG 0/8 and 0/10 too.

- **`classic`** (the default, `src/fig/FIGCarousel.cpp`): earliest deadline first. Each FIG has a deadline that every 24 ms frame reduces by 24 ms. For each FIB the FIGs are sorted by deadline, and the most overdue fills first. A deadline resets only when the FIG has sent its whole cycle. Missed deadlines are reported to the management server.
- **`classic-rate-tuning`**: the same with fixed per-FIG multipliers. Among them: FIG 0/1 and 0/2 ×0.8 (192 ms), FIG 1/1 ×0.6 (576 ms), and FIG 0/14 and 0/19 ×1.8. Both classic schedulers accept `fic-repetition-correction` (0.5 to 4) to slow all rates down for large ensembles, which can also be set live through remote control.
- **`priority`** (Maxxwave, opt-in, `src/fig/FIGCarouselPriority.cpp`): ten priority levels with poll counters that reset to 1, 1, 2, 4 … 256, and three passes per FIB: overdue FIGs, FIGs more than half way to their deadline, then anything that fits. Commit 25c38be ("CarouselPriority2", in v5.5.1) added a FIG 0/2 → FIG 1/1 phase lock and split FIG 0/8 into a programme instance at rate A and a data instance at rate B. It logs FIGs that run slower than their nominal rate.

### Rust

`dabmux/src/fic.rs` plans the FIC from the configuration, since the FIC content only changes on reload. It estimates the bytes of each part per pass and picks the shortest MCI period (96, 192, 288 or 384 ms) that leaves labels and service information their once-per-second rate. Only beyond 384 ms do those slow down, evenly, with a configuration warning. Each frame is then filled in a fixed order: FIG 0/0 and 0/7, FIG 0/10 and paced labels, steady shares for FIG 0/9 with the slower FIG 0/8 and 0/13 and for the databases, one MCI pass per planned period, and a rotation of FIG 0/5, 0/17 and 0/20 in the remaining space.

Measured over 2000 frames on DAB+ ensembles with slideshow, PTY and language per service:

| Services | MCI period | Labels, FIG 0/8, 0/9, 0/13 |
|---|---|---|
| 12, 18 | 96 ms | within 1 s |
| 24, 30 | 192 ms | within 1 s |
| 40 | 288 ms | within 1 s |
| 50 | 384 ms | about 1.1 s, with a warning |

### Differences and why

- **No choice of scheduler.** C++ chooses between algorithms; Rust has one deterministic plan. Its timing can be tested exactly and predicted from the configuration (`GET /api/config/resolved`, under `fic`). The Maxxwave scheduler was considered and not ported: its counter values and passes are tuned by trial, and its results can only be checked by running it. Rust takes its policy, labels and FIG 0/9 before PTY and language, without its mechanism.
- **MCI every 96 ms where it fits.** EN 300 401 V2.2.1 clause 6.1 asks for the complete MCI in a 96 ms period "normally". C++ uses 240 ms (192 ms with rate tuning) for every ensemble, so small ensembles get less than the standard asks. For large ensembles, Rust lengthens the MCI period before labels, because receivers abort a scan when labels time out (WorldDAB guidance on FIC generation for high service count ensembles, which the Maxxwave scheduler also follows). The upper bound, 384 ms, keeps acquisition time reasonable.
- **FIG 0/14 in the 96 ms MCI.** C++ sends it at rate B; clause 6.1 lists FIG 0/14 as MCI.
- **FIG 0/8 by component, not by service.** Clause 6.1 lets only FIG 0/8 of primary components with data applications and of data secondary components follow at the slower rate. Rust puts FIG 0/8 of secondary audio components in the MCI and the rest in the once-per-second share. The priority scheduler's split is by programme or data service, which sends slideshow primaries every 240 ms and misses secondary audio components of data services; the classic schedulers send all FIG 0/8 at rate B.
- **No FIG 0/2 → FIG 1/1 phase lock.** The MCI completes several times per label cycle in every plan, so receivers always see a service's FIG 0/2 before its label set finishes. The lock repairs a gap the Rust order does not have.
- **FIG 0/10 once per second.** C++ sends it at rate B as well; Rust sent it every 96 ms before, which cost about 80 bytes/s.
- **Monitoring.** C++ reports missed deadlines (classic) or slow rates (priority) in the log and management server. Rust measures each part's cycle and reports it in `/api/stats` (`fic_*_cycle_ms`, `fic_late_cycles`).
- **No tuning knob.** There is no `fic-repetition-correction`: the plan adapts to the configuration, so there is nothing to tune by hand.

Not addressed: the clause 6.1 recommendation to send FIG 0/8 and 0/13 of one component in the same CIF when they use the slower rate. Rust sends them in the same share, so usually close together, but not guaranteed in one CIF. Change indications after a reload add load for 5 s that the plan does not count.
