This directory holds binary resources bundled beside the yellow-vpn GUI
executable by Tauri (see the "bundle.resources" map in
src-tauri/tauri.conf.json, which stages wintun.dll flat next to the exe).
Neither file is committed to git.

wintun.dll is fetched automatically by `scripts/fetch-wintun.mjs` (wired into
the Tauri beforeDev/beforeBuild commands as `bun run fetch:wintun`). It
downloads the pinned wintun.net release, verifies its SHA-256, and extracts the
amd64 DLL here. Idempotent — skips if wintun.dll is already present.

Bundling matters because the elevated installer places wintun.dll into
C:\Program Files at install time; a non-elevated GUI cannot write there (os
error 5). The runtime downloader in wintun.rs stays as a fallback for
portable/unbundled runs.

yellow-vpn-helper.exe is produced by the `prebuild:helper` script (built from
the crates/vpn-helper workspace member) and copied here before `tauri build`.
