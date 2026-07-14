// Fetch the pinned wintun.dll and stage it as a bundled Tauri resource.
//
// The driver DLL is redistributable (WireGuard/Wintun) but not committed to git.
// Bundling it (instead of downloading on first run) means the elevated installer
// places it next to the exe at install time — otherwise a non-elevated GUI can't
// write to C:\Program Files (os error 5). The runtime downloader in wintun.rs
// stays as a fallback for portable/unbundled runs.
//
// Idempotent: skips work if resources/wintun.dll is already present.
//
// Usage: node scripts/fetch-wintun.mjs
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateRawSync } from "node:zlib";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dest = join(root, "src-tauri", "resources", "wintun.dll");

if (existsSync(dest)) {
  console.log(`wintun.dll already staged: ${dest}`);
  process.exit(0);
}

// Pinned release archive + its SHA-256, mirroring the constants in
// src-tauri/src/wintun.rs. The DLL is loaded into the elevated helper, so the
// archive must be integrity-checked, not just fetched over TLS.
const URL = "https://www.wintun.net/builds/wintun-0.14.1.zip";
const ZIP_SHA256 = "07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51";
const ENTRY = "wintun/bin/amd64/wintun.dll"; // x86_64 — the only shipping target

const resp = await fetch(URL);
if (!resp.ok) {
  console.error(`download failed: ${resp.status} ${resp.statusText}`);
  process.exit(1);
}
const zip = Buffer.from(await resp.arrayBuffer());

const got = createHash("sha256").update(zip).digest("hex");
if (got !== ZIP_SHA256) {
  console.error(`wintun archive integrity check failed (sha256 ${got}, expected ${ZIP_SHA256})`);
  process.exit(1);
}

// Minimal ZIP extraction of a single known entry (no dependency): locate the
// entry in the End-of-Central-Directory -> central directory, then read + inflate
// its local file record.
function extract(buf, name) {
  // Find End of Central Directory record (signature 0x06054b50), scanning back.
  let eocd = -1;
  for (let i = buf.length - 22; i >= 0; i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) { eocd = i; break; }
  }
  if (eocd < 0) throw new Error("EOCD not found");
  let off = buf.readUInt32LE(eocd + 16); // central directory offset
  const count = buf.readUInt16LE(eocd + 10);
  for (let i = 0; i < count; i++) {
    if (buf.readUInt32LE(off) !== 0x02014b50) throw new Error("bad central header");
    const method = buf.readUInt16LE(off + 10);
    const compSize = buf.readUInt32LE(off + 20);
    const nameLen = buf.readUInt16LE(off + 28);
    const extraLen = buf.readUInt16LE(off + 30);
    const commentLen = buf.readUInt16LE(off + 32);
    const localOff = buf.readUInt32LE(off + 42);
    const entryName = buf.toString("utf8", off + 46, off + 46 + nameLen);
    if (entryName === name) {
      // Parse local file header to skip its (possibly different) name/extra fields.
      if (buf.readUInt32LE(localOff) !== 0x04034b50) throw new Error("bad local header");
      const lNameLen = buf.readUInt16LE(localOff + 26);
      const lExtraLen = buf.readUInt16LE(localOff + 28);
      const dataOff = localOff + 30 + lNameLen + lExtraLen;
      const data = buf.subarray(dataOff, dataOff + compSize);
      return method === 8 ? inflateRawSync(data) : Buffer.from(data);
    }
    off += 46 + nameLen + extraLen + commentLen;
  }
  throw new Error(`entry '${name}' not found in archive`);
}

const dll = extract(zip, ENTRY);
mkdirSync(dirname(dest), { recursive: true });
writeFileSync(dest, dll);
console.log(`staged resource: ${dest} (${dll.length} bytes)`);
