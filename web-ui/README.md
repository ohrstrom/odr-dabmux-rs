# web-ui

To install dependencies:

```bash
bun install
```

To run against a mux on port 7777:

```bash
bun run dev        # or: DABMUX_API=http://127.0.0.1:7778 PORT=3001 bun run dev
```

To ship it inside the mux binary, build `dist/` before the release build; dabmux then serves the UI on `/`:

```bash
bun run build && cargo build --release
```

Debug builds of dabmux read `dist/` from disk, so a rebuilt UI shows after a browser reload.

This project was created using `bun init` in bun v1.4.2. [Bun](https://bun.com) is a fast all-in-one JavaScript runtime.
