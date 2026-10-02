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
