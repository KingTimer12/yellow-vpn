# Yellow VPN

A cross-platform VPN client for **Windows**, **macOS**, **Linux**, and
**Android**. Yellow VPN speaks two enterprise VPN protocols — **Cisco
AnyConnect** and **Checkpoint** (CCC) — behind a modern, native-feeling UI.

Built with a [Tauri v2](https://tauri.app/) shell, a React 19 + TypeScript
frontend, and a Rust backend. The networking core is written from scratch in
Rust: protocol clients, the TUN device, routing, and the reconnect/supervision
lifecycle. The same Rust engine drives every platform — on desktop through an
elevated helper, on Android through a JNI bridge into a `VpnService`.

---

## Features

- **Two enterprise protocols** — Cisco AnyConnect and Checkpoint CCC (auth,
  cipher, framing, and session handling implemented natively in Rust).
- **Cross-platform** — one codebase, native TUN + routing on Windows, macOS,
  Linux, and Android.
- **Least-privilege by design** — on desktop the UI runs unprivileged; only a
  small, short-lived helper is elevated, and only when you connect. On Android
  the OS `VpnService` consent gate replaces elevation entirely.
- **Profiles** — connection profiles are stored in a per-user SQLite database
  in the OS app-data directory.
- **Automatic reconnect** — the engine supervises the tunnel and reconnects
  with backoff on transient failures.
- **Native elevation prompts** — UAC on Windows, the authorization dialog on
  macOS, and a polkit prompt on Linux. No password ever passes through the UI.

---

## Architecture

On **desktop**, Yellow VPN splits into three processes around a single design
constraint: **the UI must never run as root.** (Android has no privilege split —
see [Android](#android) below.)

```
┌──────────────────────┐        local transport         ┌──────────────────────┐
│  GUI (unprivileged)   │  ───────────────────────────▶ │  Helper (elevated)    │
│  Tauri + React        │   newline-delimited JSON       │  root / Administrator │
│  profiles DB, tray,   │  ◀─────────────────────────── │  owns the VPN engine  │
│  UI                   │                                └───────────┬──────────┘
└──────────────────────┘                                            │ consumes
                                                        ┌───────────▼──────────┐
                                                        │  Engine (library)     │
                                                        │  protocols, TUN,      │
                                                        │  routing, lifecycle   │
                                                        └──────────────────────┘
```

1. **GUI** (`src-tauri`) — the Tauri app. Owns the profiles SQLite DB, the tray,
   and the UI. It cannot touch TUN or routing; it only talks to the helper.
2. **Elevated helper** (`crates/vpn-helper`) — owns the VPN engine and is
   elevated on connect. On Unix it is *one-shot*: it serves a single connection,
   then exits.
3. **Engine** (`crates/vpn-engine`) — the networking library the helper drives.
   Protocol clients, the TUN device, routing, and reconnect/supervision.

### IPC boundary (`crates/vpn-ipc`)

The GUI and helper communicate over a newline-delimited JSON protocol. The type
surface is identical across platforms; only the transport differs:

- **Windows** — named pipe `\\.\pipe\yellow-vpn`.
- **macOS / Linux** — Unix domain socket at `/var/run/yellow-vpn/helper.sock`.
  Root binds it, then chowns it to the interactive user with mode `0600`, so the
  control channel is locked to that user and root.

### Android (single process)

Android has no elevated helper and no IPC socket. The app is one process; the
same `vpn-engine` crate is compiled as a `cdylib` (`libvpn_engine.so`) and
loaded into the app via JNI. A Tauri mobile plugin (Kotlin, in
`src-tauri/gen/android/app/src/main/java/app/yellowvpn/plugin/`) bridges the UI
to the engine:

- **`VpnBridge`** — `System.loadLibrary("vpn_engine")` + the `runEngine` /
  `stopEngine` JNI entry points (Rust side: `crates/vpn-engine/src/jni_bridge.rs`).
- **`YellowVpnService`** — an Android `VpnService` (foreground service). It owns
  the TUN, established from `VpnService.Builder`. The tunnel is built *after* the
  handshake, using the **server-assigned** address/DNS, so return routing is
  correct.
- **`VpnPlugin`** — the `@TauriPlugin`; UI-side Tauri commands invoke it through
  `run_mobile_plugin` (JS plugin access is ACL-gated, so the app-command layer
  proxies instead). UI connection state is polled via the `vpn_status` command.

Elevation is replaced by the system `VpnService` consent dialog on first
connect. Routing is owned by `VpnService.Builder` (the engine's desktop
`RoutingGuard` is a no-op on Android).

---

## Platform support

| Platform | Elevation             | TUN                       | Notes                                            |
| -------- | --------------------- | ------------------------- | ------------------------------------------------ |
| Windows  | UAC prompt            | `wintun.dll`              | `wintun.dll` is auto-downloaded on first run.    |
| macOS    | Authorization dialog  | built-in `utun`           | Ad-hoc signed; see `docs/macos-signing.md`.      |
| Linux    | `pkexec` (polkit)     | `/dev/net/tun`            | Requires `polkit` / `pkexec` installed.          |
| Android  | `VpnService` consent  | `VpnService.Builder`      | Sideload APK; engine via JNI. See below.         |

### Windows

The Windows TUN adapter is provided by `wintun.dll`. Yellow VPN downloads it on
first run (see `src-tauri/src/wintun.rs`) — no manual install needed. Connecting
triggers a standard UAC prompt to launch the elevated helper.

### macOS

macOS uses its built-in `utun` interface and shows the native authorization
dialog on connect. Development builds are ad-hoc signed; see
[`docs/macos-signing.md`](docs/macos-signing.md) for signing and distribution
details.

### Linux

The helper is elevated with `pkexec` (polkit), which renders your desktop's
native authentication dialog. On a standard GNOME/KDE desktop this works out of
the box — just ensure `polkit` / `pkexec` is installed:

- Debian/Ubuntu: `sudo apt install policykit-1` (or `pkexec` on newer releases)
- Fedora/RHEL: `sudo dnf install polkit`
- Arch: `sudo pacman -S polkit`

#### Running under WSL2

WSL2 has no graphical polkit agent and usually no `systemd-logind` session, so
`pkexec` falls back to a terminal prompt and often fails with
`No session for cookie`. Two dev workarounds:

**A. Passwordless pkexec for the helper (recommended).** Add a polkit rule that
skips auth for *your user* running *your* built helper. Returning `YES` also
avoids the missing-session problem. Replace `YOUR_USER` and the path with yours:

```bash
sudo tee /etc/polkit-1/rules.d/49-yellowvpn-dev.rules >/dev/null <<'EOF'
polkit.addRule(function(action, subject) {
    if (action.id == "org.freedesktop.policykit.exec" &&
        subject.user == "YOUR_USER" &&
        action.lookup("program") ==
            "/absolute/path/to/YellowVPN/target/debug/yellow-vpn-helper") {
        return polkit.Result.YES;
    }
});
EOF
```

polkit hot-reloads `rules.d`, so no restart is needed. **Dev only** — remove
before shipping: `sudo rm /etc/polkit-1/rules.d/49-yellowvpn-dev.rules`.

The `program` path must match whatever `pkexec` actually launches, which depends
on how you started the app. An **installed** build uses the install path
(e.g. `/usr/bin/yellow-vpn-helper`); a `cargo`/dev run uses
`target/debug/yellow-vpn-helper`. If you use both, match both:

```js
        (action.lookup("program") == "/usr/bin/yellow-vpn-helper" ||
         action.lookup("program") ==
            "/absolute/path/to/YellowVPN/target/debug/yellow-vpn-helper")
```

**B. Start the helper manually.** The GUI connects to an existing helper socket
before spawning one, so you can pre-start it (pass your uid, which locks the
socket to you):

```bash
sudo ./target/debug/yellow-vpn-helper $(id -u)
```

The helper is one-shot: it serves a single connection then exits, so restart it
for each connect.

Full VPN testing under WSL2 is unreliable anyway (no TUN device by default, no
real desktop) — a real Linux desktop is the intended target.

#### WSL2 rendering warnings

WSL2 has no real GPU, so WebKitGTK spams `libEGL` / `MESA` / `ZINK` warnings and
may fail to render the window (`Gtk-CRITICAL ... GTK_IS_WIDGET`). Force software
rendering:

```bash
WEBKIT_DISABLE_COMPOSITING_MODE=1 LIBGL_ALWAYS_SOFTWARE=1 yellow-vpn
```

If the window still doesn't appear, also set `WEBKIT_DISABLE_DMABUF_RENDERER=1`.
These are WSL-only quirks; they don't occur on a real desktop.

### Android

Android runs the same Rust engine, compiled to a native library and loaded into
a `VpnService` over JNI — there is no elevated helper. On first connect the
system shows the standard VPN-consent dialog; a persistent foreground
notification keeps the tunnel alive in the background. Both protocols
(AnyConnect and Checkpoint) are supported.

Distribution is by **sideloaded APK** (not published to the Play Store).

**Build prerequisites** (in addition to the [general ones](#prerequisites)):

- Android SDK + NDK (tested with NDK `27.1.12297006`) and JDK 17
- [`cargo-ndk`](https://github.com/bbqsrc/cargo-ndk): `cargo install cargo-ndk`
- Rust Android targets:
  `rustup target add aarch64-linux-android x86_64-linux-android`

**Build & run:**

```bash
bun run android:engine   # cross-compile vpn-engine (.so) into jniLibs via cargo-ndk
bun run android:dev      # run on a connected device / emulator
bun run android:build    # build the release APK
```

`android:dev` / `android:build` also rebuild the engine first. Only the two ABIs
above are produced (`arm64-v8a`, `x86_64`); the generated `.so` files and the
APK are **not** checked into git.

> **Known A1 limitations:** full-tunnel only (no split-tunnel routes yet); the
> engine's own sockets are kept off the tunnel via `addDisallowedApplication`
> (a stand-in for per-socket `protect()`); UI state is polled rather than pushed.

---

## Getting started

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2024, toolchain 1.88+)
- [Bun](https://bun.sh/) — the package manager for this project
- Platform build dependencies for Tauri v2
  ([see the Tauri prerequisites](https://tauri.app/start/prerequisites/))
- Linux only: `polkit` / `pkexec` (see [above](#linux))

### Install & run

```bash
bun install          # install frontend dependencies
bun run tauri dev    # build + stage the helper, then launch the full app
```

`bun run tauri dev` runs the `predev:helper` hook, which builds `vpn-helper` and
stages it as a Tauri sidecar next to the GUI executable. A bare `cargo build`
does **not** restage the helper — if you change helper code, re-run
`bun run tauri dev` (or `node scripts/prepare-helper.mjs`).

---

## Development

### Common commands

```bash
bun run dev            # Bun HTML dev server + HMR on :1420 (frontend only)
bun run tauri dev      # full app (builds + stages helper, then runs the GUI)
bun run build          # tsc typecheck + Bun bundle -> dist/ (frontend only)
bun run tauri:build    # release bundle (stages helper --release, then tauri build)

cargo build            # build all Rust workspace crates
cargo test -p vpn-engine   # engine tests
cargo test -p vpn-ipc      # IPC wire-type tests
cargo test <name>          # run a single test by name
```

### Workspace layout

```
src-tauri/            GUI (Tauri app): commands, profiles DB, wintun, pipe client
  gen/android/        generated Android project + Kotlin VPN plugin (VpnService)
crates/
  vpn-engine/         networking core: protocols, TUN, routing, lifecycle
    platform/         per-OS TUN + routing (windows.rs, macos.rs, linux.rs, android.rs)
    checkpoint/       Checkpoint CCC protocol (auth, cipher, framing, session)
    jni_bridge.rs     Android JNI entry points (runEngine / stopEngine)
  vpn-helper/         elevated helper binary (drives the engine; desktop only)
  vpn-ipc/            the GUI↔helper wire contract (no async/engine deps)
src/                  React + TypeScript frontend
scripts/
  prepare-helper.mjs      builds vpn-helper and stages it as a Tauri sidecar
  build-android-engine.mjs builds vpn-engine (.so) into jniLibs via cargo-ndk
docs/                 macOS signing notes + design specs/plans
```

> **Note:** the IPC wire types in `crates/vpn-ipc` are mirrored by hand in
> `src/lib/vpn.ts`. Change one side, change the other.

### Design docs

`docs/superpowers/specs/` and `docs/superpowers/plans/` hold the
VPN-integration and profiles-UI design records. Read these before large changes
to the connection lifecycle or profiles.

---

## License

Licensed under the [MIT License](LICENSE).
