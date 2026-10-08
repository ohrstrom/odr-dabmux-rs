# Web UI

Keep this frontend deliberately minimal.

- Use Bun for package management, development and builds.
- Use Bun's native HTML bundler. Do not introduce Vite, webpack, PostCSS,
  Next.js, or another frontend build system.
- Keep the flat project structure. Do not introduce a `src/` directory or
  additional architectural layers without a concrete need.
- The backend is the Rust application. Do not add a Bun/Node application
  server.
- Use relative `/api/...` URLs for backend communication. In development,
  `dev.ts` serves the UI and forwards `/api/*` to the mux (`DABMUX_API`,
  default `http://127.0.0.1:7777`); it is dev tooling, not an app server.
- Prefer native `fetch` and plain React. Do not add Axios, state-management
  libraries, or data-fetching frameworks unless their complexity is actually
  needed.
- Use existing shadcn components where appropriate. Add components with
  `bunx shadcn@latest add <component>`.
- Do not run `shadcn init`; this project is manually configured because
  shadcn does not detect Bun's native HTML setup.
- Keep the frontend buildable as static assets. The production build is
  embedded in the Rust binary (`dabmux/src/web_ui.rs`, served on `/`).

## Commands

```sh
bun run dev                     # bun --hot ./dev.ts, port 3000 (PORT=…)
bunx shadcn@latest add <component>
bun run build                   # build.ts: dist/, Tailwind processed
```