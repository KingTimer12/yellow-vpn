// Production frontend bundle via Bun's bundler (replaces `vite build`).
// Bundles the HTML entrypoint — JS/TSX, CSS (Tailwind v4 via bun-plugin-tailwind),
// and static assets — into ../dist, which Tauri picks up as `frontendDist`.
//
// tsconfig `paths` ("@/*" -> ./src/*) is honored by Bun natively, so no alias
// config is needed here (Vite required it in vite.config.ts).
import { rm } from "node:fs/promises";
import tailwind from "bun-plugin-tailwind";

const outdir = "./dist";

// Clean stale hashed assets so old chunks don't accumulate across builds.
await rm(outdir, { recursive: true, force: true });

const result = await Bun.build({
  entrypoints: ["./index.html"],
  outdir,
  minify: true,
  target: "browser",
  sourcemap: "none",
  plugins: [tailwind],
  // NOTE: Bun's CSS bundler inlines small assets referenced by url() as data:
  // URIs and (as of 1.3.14) exposes no option to force them external — the
  // esbuild-style "file"/"copy" loaders aren't implemented for CSS url() yet.
  // So the 4 woff2 font subsets end up inlined in the stylesheet. For a Tauri
  // app this is harmless: every asset is already a local file, so there are no
  // extra network round-trips to save.
  define: { "process.env.NODE_ENV": '"production"' },
});

if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}

const total = result.outputs.reduce((n, o) => n + o.size, 0);
console.log(`bun build: ${result.outputs.length} files, ${(total / 1024).toFixed(1)} KiB total`);
