// Development server only: serves the UI with hot reloading and forwards
// /api/* to the running mux, so the app can keep using relative URLs.
// Production builds are static files served by the Rust binary.
import index from "./index.html"

const api = process.env.DABMUX_API ?? "http://127.0.0.1:7777"

const server = Bun.serve({
  port: Number(process.env.PORT ?? 3000),
  development: { hmr: true, console: true },
  routes: {
    "/api/*": (request) => {
      const url = new URL(request.url)
      return fetch(new URL(url.pathname + url.search, api), {
        method: request.method,
        headers: request.headers,
        body: request.body,
      }).catch(
        (error) =>
          new Response(
            JSON.stringify({ error: `mux API at ${api} unreachable: ${error}` }),
            { status: 502, headers: { "content-type": "application/json" } }
          )
      )
    },
    "/*": index,
  },
})

console.log(`web-ui on ${server.url}, API forwarded to ${api}`)
