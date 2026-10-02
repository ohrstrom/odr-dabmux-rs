// Production build: static files in dist/, with Tailwind processed (the
// bunfig.toml plugin only applies to the dev server).
import tailwind from "bun-plugin-tailwind"

const result = await Bun.build({
  entrypoints: ["./index.html"],
  outdir: process.argv[2] ?? "dist",
  minify: true,
  plugins: [tailwind],
})
for (const log of result.logs) console.log(log)
if (!result.success) process.exit(1)
for (const output of result.outputs) console.log(output.path, output.size)
