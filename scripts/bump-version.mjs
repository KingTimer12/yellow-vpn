// Bump the app version across config files.
// Usage: bun scripts/bump-version.mjs <version>
import { readFileSync, writeFileSync } from "node:fs";

const version = process.argv[2]?.replace(/^v/, "").trim();
if (!version) {
  console.error("usage: bump-version.mjs <version>");
  process.exit(1);
}
if (!/^\d+\.\d+\.\d+/.test(version)) {
  console.error(`invalid version: "${version}"`);
  process.exit(1);
}

function bumpJson(path, key = "version") {
  const json = JSON.parse(readFileSync(path, "utf8"));
  json[key] = version;
  writeFileSync(path, JSON.stringify(json, null, 2) + "\n");
  console.log(`${path} -> ${version}`);
}

// src-tauri/Cargo.toml: replace ONLY the package-level `version = "..."`
// (line-start; dependency versions live inside inline tables, not at col 0).
function bumpCargo(path) {
  const text = readFileSync(path, "utf8");
  const next = text.replace(/^version = "[^"]*"/m, `version = "${version}"`);
  if (next === text) {
    console.warn(`${path}: no package version line matched (skipped)`);
    return;
  }
  writeFileSync(path, next);
  console.log(`${path} -> ${version}`);
}

bumpJson("src-tauri/tauri.conf.json");
bumpJson("package.json");
bumpCargo("src-tauri/Cargo.toml");
