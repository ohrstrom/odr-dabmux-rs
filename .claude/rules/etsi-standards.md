# ETSI standards

Use the downloaded ETSI standards rather than recalling spec details from memory.

- PDFs and a plain-text extraction of each are in `docs/etsi/pdf/` (gitignored). Grep the `.txt`; read the PDF when figures or tables don't survive extraction.
- Naming: `ETSI_<EN|TS>_<number with underscores>_V<x.y.z>_<short_topic>.{pdf,txt}`, for example `ETSI_EN_300_401_V2.2.1_DAB_system.pdf`. Don't keep ETSI's original names such as `en_300401v020201p.pdf`.
- Present: EN 300 401 V2.2.1 (DAB, 2026-06), TS 101 756 (registered tables), TS 102 818 and TS 102 371 (SPI), EN 301 234 (MOT), TS 101 499 (MOT slideshow), TS 102 693 (EDI), TS 102 563 (DAB+), TS 102 821 (DCP), TS 102 980 (DL Plus), TS 103 176 V2.6.1 (service linking and frequency information, 2026-07), TR 101 496-2 (system feature guidelines). `docs/etsi/README.md` links the sources.
- EN 300 401 V2.2.1 is newer than the V2.1.1 the C++ reference follows, and TS 103 176 V2.6.1 newer than its service following. Where they differ, follow the standard and note it; `devsupport/cpp-reference/README.md` lists the known C++ departures.

Downloading more: ETSI answers 403 to plain `curl`, so send a browser `User-Agent`. `https://www.etsi.org/deliver/<etsi_en|etsi_ts>/<range>/<number>/` lists the versions; the PDF is in the `<version>_60/` directory. Extract text with `uv run --with pypdf python`.
