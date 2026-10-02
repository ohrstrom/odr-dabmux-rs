# Web UI

Keep this frontend deliberately minimal.

- Use Bun for package management, development and builds.
- Use Bun's native HTML bundler. Do not introduce Vite, webpack, PostCSS,
  Next.js, or another frontend build system.
- Keep the flat project structure. Do not introduce a `src/` directory or
  additional architectural layers without a concrete need.
- The backend is the Rust application. Do not add a Bun/Node application
  server.
- Use relative `/api/...` URLs for backend communication.
- Prefer native `fetch` and plain React. Do not add Axios, state-management
  libraries, or data-fetching frameworks unless their complexity is actually
  needed.
- Use existing shadcn components where appropriate. Add components with
  `bunx shadcn@latest add <component>`.
- Do not run `shadcn init`; this project is manually configured because
  shadcn does not detect Bun's native HTML setup.
- Keep the frontend buildable as static assets. The production build will
  eventually be embedded in the Rust binary.

## Commands

```sh
bun ./index.html
bunx shadcn@latest add <component>
bun build ./index.html --outdir=dist --minify
```