# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Yellow VPN — a cross-platform (Windows/macOS/Linux desktop + Android) VPN client. Tauri v2
shell, React 19 + TypeScript frontend, Rust backend. Speaks two enterprise VPN protocols:
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

bun run android:engine      # build vpn-engine (.so) into jniLibs via cargo-ndk
bun run android:dev         # run on device/emulator (rebuilds engine first)
bun run android:build       # release APK (rebuilds engine first)
```

Android needs `cargo-ndk` (`cargo install cargo-ndk`, NOT a Cargo dep), the Android NDK
(tested 27.1.12297006), JDK 17, and the `aarch64-linux-android` + `x86_64-linux-android` Rust
targets. `scripts/build-android-engine.mjs` cross-compiles `vpn-engine` as a `cdylib`
(`libvpn_engine.so`) into `src-tauri/gen/android/app/src/main/jniLibs/`. The `.so` files and the
built APK are gitignored — never commit `gen/android/{app/build,build,buildSrc/build,.gradle}` or
`jniLibs/**/*.so`.

The helper binary is **not** built by cargo automatically for the app — `scripts/prepare-helper.mjs`
builds `vpn-helper` and stages it as a Tauri sidecar (`yellow-vpn-helper-<target-triple>`) next to
the GUI exe. This runs via the `predev:helper` / `prebuild:helper` npm hooks. If you change helper
code, re-run `bun run tauri dev` (or `node scripts/prepare-helper.mjs`) — a bare `cargo build` won't
restage it.

## Architecture

### Desktop (Windows/macOS/Linux)

Three-process privilege split — the core design constraint:

1. **GUI (`src-tauri`, unprivileged)** — Tauri app. Owns the profiles SQLite DB (rusqlite), the tray,
   and the UI. Cannot touch TUN/routing. Talks to the helper over a local transport.
2. **Elevated helper (`crates/vpn-helper`, root/Administrator)** — owns the VPN engine. Elevated on
   connect: UAC (Windows) / `osascript` auth dialog (macOS) / `pkexec` polkit (Linux). One-shot on
   Unix (serves a single connection then exits).
3. **Engine (`crates/vpn-engine`)** — library, consumed by the helper. Protocol clients, TUN device,
   routing, reconnect/supervision lifecycle. `platform/{windows,macos,linux}.rs` split OS-specific
   TUN+routing; `checkpoint/` holds the Checkpoint CCC protocol (auth, cipher, framing, session).

### Android (single process, no privilege split)

No helper, no IPC socket. The same `vpn-engine` crate compiles as a `cdylib` (`crate-type =
["lib", "cdylib"]`) and is loaded into the app process via JNI. Key pieces:

- `crates/vpn-engine/src/jni_bridge.rs` — `Java_app_yellowvpn_plugin_VpnBridge_runEngine` /
  `stopEngine` exports (edition 2024 → `#[unsafe(no_mangle)]`). Runs a **current-thread** tokio
  runtime because `JNIEnv` is `!Send`. State delivered by re-attaching the thread to the JVM and
  calling `StateCallback.onState(String)`.
- `crates/vpn-engine/src/client.rs` — `run_client_supervised_android`; a task-local
  `ANDROID_TUN_FACTORY` injects a closure that builds the TUN **after** the handshake with the
  server-assigned address (Office-Mode) via the Kotlin `TunBuilder.configure(address, mtu, dns)`.
- `crates/vpn-engine/src/routing.rs` — `RoutingGuard` is a no-op on Android (`VpnService.Builder`
  owns routing). `tun_device::open_tun_from_fd` wraps the fd from `Builder.establish()`.
- Kotlin plugin (`src-tauri/gen/android/app/src/main/java/app/yellowvpn/plugin/`): `VpnBridge`
  (`System.loadLibrary`), `YellowVpnService` (VpnService + foreground notification, generation guard
  so a replaced engine thread can't tear down the new tunnel), `VpnPlugin` (`@TauriPlugin`),
  `VpnController`.

**Two ACL/init gotchas that shaped this design:** (1) Tauri does NOT initialize `ndk_context`, so a
Rust→Kotlin `ndk_context` bridge panics — use a Tauri mobile plugin instead. (2) JS-side plugin
`invoke`/listeners are ACL-gated (local plugins lack permission manifests → "not allowed / Plugin
not found"), so `src-tauri/src/lib.rs` app commands (`vpn_connect`/`vpn_disconnect`/`vpn_status`)
proxy to the plugin via `handle.run_mobile_plugin(...)` (Rust-side, NOT ACL-gated). UI state is
**polled** through `vpn_status` (~1.2s); no push events. `src-tauri/tauri.android.conf.json` drops
the helper sidecar (`bundle.externalBin: []`).

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
with `invoke()`. `src/hooks/useVpnState.ts` drives connection state — desktop listens to the
`vpn://state` event; mobile polls `vpn_status` (branch on `IS_MOBILE`/`useIsMobile`, userAgent).
`useWintun.ts` handles the Windows wintun.dll first-run download gate (`SetupGate.tsx`). UI is
shadcn/Radix + Tailwind v4 + framer-motion. On mobile, `ProfileDialog` renders a shadcn/vaul
`Drawer` instead of a `Dialog`, and `App.tsx` drops desktop window chrome (title bar, window
controls, rounded border) + adds an optimistic "Connecting" state (polling can miss a fast connect).
Profiles DB lives per-user in the OS app-data dir (see `db_path()` in `lib.rs`).

## Platform notes

- **Windows**: needs `wintun.dll` (auto-downloaded on first run, see `src-tauri/src/wintun.rs`).
- **Linux**: needs `polkit`/`pkexec`. Under WSL2 pkexec fails (`No session for cookie`) — see README
  for the dev polkit-rule workaround or manual `sudo ./target/debug/yellow-vpn-helper $(id -u)`.
- **macOS**: ad-hoc signing + auth dialog; see `docs/macos-signing.md`.
- **Android**: no elevation — `VpnService` consent dialog on first connect + foreground service.
  Engine loaded via JNI (not `ndk_context`). Full-tunnel only for now (A1); split-tunnel, per-socket
  `protect()`, and pushed state events are documented A2+ follow-ups.

## Design docs

`docs/superpowers/specs/` and `docs/superpowers/plans/` hold the VPN-integration, profiles-UI, and
Android-support design records — read these before large changes to the connection lifecycle,
profiles, or the Android bridge.
