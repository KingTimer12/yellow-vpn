# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Yellow VPN — a cross-platform (Windows/macOS/Linux) desktop VPN client. Tauri v2 shell,
React 19 + TypeScript frontend, Rust backend. Speaks two enterprise VPN protocols:
**AnyConnect** and **Checkpoint**. Package manager is **bun**.

## Commands

```bash
bun install                 # deps
bun run dev                 # vite dev server (frontend only)
bun run tauri dev           # full app: builds+stages helper (predev:helper), runs GUI
bun run tauri:build         # release bundle (prebuild:helper --release, then tauri build)
bun run build               # tsc + vite build (frontend bundle only)

cargo build                 # all workspace crates
cargo test -p vpn-engine    # engine tests
cargo test -p vpn-ipc       # IPC wire-type tests
cargo test <name>           # single test by name
```

The helper binary is **not** built by cargo automatically for the app — `scripts/prepare-helper.mjs`
builds `vpn-helper` and stages it as a Tauri sidecar (`yellow-vpn-helper-<target-triple>`) next to
the GUI exe. This runs via the `predev:helper` / `prebuild:helper` npm hooks. If you change helper
code, re-run `bun run tauri dev` (or `node scripts/prepare-helper.mjs`) — a bare `cargo build` won't
restage it.

## Architecture

Three-process privilege split — the core design constraint:

1. **GUI (`src-tauri`, unprivileged)** — Tauri app. Owns the profiles SQLite DB (rusqlite), the tray,
   and the UI. Cannot touch TUN/routing. Talks to the helper over a local transport.
2. **Elevated helper (`crates/vpn-helper`, root/Administrator)** — owns the VPN engine. Elevated on
   connect: UAC (Windows) / `osascript` auth dialog (macOS) / `pkexec` polkit (Linux). One-shot on
   Unix (serves a single connection then exits).
3. **Engine (`crates/vpn-engine`)** — library, consumed by the helper. Protocol clients, TUN device,
   routing, reconnect/supervision lifecycle. `platform/{windows,macos,linux}.rs` split OS-specific
   TUN+routing; `checkpoint/` holds the Checkpoint CCC protocol (auth, cipher, framing, session).

### IPC boundary (`crates/vpn-ipc`)

The GUI↔helper contract. Newline-delimited JSON, no async/engine deps. Transport is per-OS but the
type surface is identical:
- **Windows**: named pipe `\\.\pipe\yellow-vpn`.
- **macOS/Linux**: Unix socket `/var/run/yellow-vpn/helper.sock` (root binds it, chowns to the
  interactive user, mode 0600 — locked to that user + root).

Key types: `ClientCommand` (Connect/Disconnect/Shutdown), `ClientMessage` (State/Error/Bye),
`WireConfig`, `WireState`. **These types are mirrored by hand in `src/lib/vpn.ts`** — change one side,
change the other. `src-tauri/src/pipe.rs` is the GUI-side client + privileged-spawn logic;
`vpn-helper/src/main.rs` (`mod proto`) is the transport-agnostic serving logic.

### Frontend

Tauri commands (defined in `src-tauri/src/lib.rs`) are the only bridge; `src/lib/vpn.ts` wraps them
with `invoke()`. `src/hooks/useVpnState.ts` drives connection state (listens for emitted events),
`useWintun.ts` handles the Windows wintun.dll first-run download gate (`SetupGate.tsx`). UI is
shadcn/Radix + Tailwind v4 + framer-motion. Profiles DB lives per-user in the OS app-data dir (see
`db_path()` in `lib.rs`).

## Platform notes

- **Windows**: needs `wintun.dll` (auto-downloaded on first run, see `src-tauri/src/wintun.rs`).
- **Linux**: needs `polkit`/`pkexec`. Under WSL2 pkexec fails (`No session for cookie`) — see README
  for the dev polkit-rule workaround or manual `sudo ./target/debug/yellow-vpn-helper $(id -u)`.
- **macOS**: ad-hoc signing + auth dialog; see `docs/macos-signing.md`.

## Design docs

`docs/superpowers/specs/` and `docs/superpowers/plans/` hold the VPN-integration and profiles-UI
design records — read these before large changes to the connection lifecycle or profiles.
