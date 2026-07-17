// Build the elevated VPN helper and stage it as a Tauri sidecar (externalBin).
//
// Tauri expects sidecar binaries named `<name>-<target-triple>` (with `.exe` on
// Windows) and places them next to the main executable in the bundle — which is
// exactly where the GUI's `helper_path()` looks for `yellow-vpn-helper` at
// runtime. Works on macOS, Linux, and Windows.
//
// Usage: node scripts/prepare-helper.mjs [--release]
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const release = process.argv.includes("--release");

// Host target triple, e.g. `aarch64-apple-darwin`.
const rustcOut = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
const hostTriple = rustcOut.match(/^host:\s*(.+)$/m)?.[1]?.trim();
if (!hostTriple) {
  console.error("could not determine host target triple from `rustc -vV`");
  process.exit(1);
}

// The sidecar MUST match the arch Tauri is bundling for. When cross-compiling
// (e.g. building the x86_64 macOS bundle on an arm64 runner) the CI passes
// HELPER_TARGET so the helper is built for the target triple, not the host —
// otherwise Tauri looks for `yellow-vpn-helper-<target>` and finds nothing (or
// a wrong-arch binary). Defaults to the host triple for a normal native build.
const triple = process.env.HELPER_TARGET?.trim() || hostTriple;

const isWindows = triple.includes("windows");
const exeSuffix = isWindows ? ".exe" : "";
const profileDir = release ? "release" : "debug";

// Build the helper for the resolved triple. Always passing --target keeps the
// output path predictable (target/<triple>/<profile>/) on host and cross builds.
execFileSync(
  "cargo",
  ["build", "-p", "vpn-helper", "--target", triple, ...(release ? ["--release"] : [])],
  { stdio: "inherit", cwd: root },
);

const src = join(root, "target", triple, profileDir, `yellow-vpn-helper${exeSuffix}`);
const destDir = join(root, "src-tauri", "binaries");
const dest = join(destDir, `yellow-vpn-helper-${triple}${exeSuffix}`);

mkdirSync(destDir, { recursive: true });
copyFileSync(src, dest);
console.log(`staged sidecar: ${dest}`);
