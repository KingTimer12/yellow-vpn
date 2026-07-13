# Android Support — Design (A1: Porting Foundation / MVP)

**Date:** 2026-07-13
**Status:** Design approved, pending spec review
**End goal:** Full feature parity with the desktop client on Android.
**This spec covers:** Sub-project **A1**, the porting foundation (MVP). Later
sub-projects (A2–A4) are captured as a roadmap at the end; each gets its own
spec + plan.

## Why Android is a re-architecture, not a build fix

The desktop client is built on a three-process privilege split: an unprivileged
GUI, an elevated helper (root/Administrator), and the engine as a library the
helper drives, talking over a named pipe / Unix socket (`vpn-ipc`). **None of
that model exists on Android:**

- No root, no elevated child process, no UAC/pkexec/osascript.
- No direct `/dev/net/tun`. The only sanctioned path to a tunnel is the system
  `VpnService` API, which hands the app an already-open TUN file descriptor
  after the user grants consent.
- No Tauri sidecar as a separate OS process. The engine must run **in-process**
  as a native library (`.so`) loaded into the app.
- Routing is configured declaratively through `VpnService.Builder`, not by
  issuing route commands like `platform/{linux,macos,windows}.rs` do.

Consequently the `vpn-helper` binary, the `vpn-ipc` transport, and the elevation
logic in `pipe.rs` have **no Android counterpart**. What ports cleanly is the
protocol core: the AnyConnect and Checkpoint clients, framing, cipher, auth, and
the supervision lifecycle — they are socket + crypto logic, platform-agnostic.

The current build failure (`resource path
binaries/yellow-vpn-helper-aarch64-linux-android doesn't exist`) is just the
first symptom: `prepare-helper.mjs` only stages a host-triple sidecar, and a
sidecar is the wrong model for Android entirely.

## Decisions locked in brainstorming

- **Bridge:** Tauri mobile plugin (Kotlin) hosting the `VpnService`, bridged to
  `vpn-engine` (compiled as a `cdylib` `.so`) over JNI. Stays inside the Tauri
  ecosystem, reuses the existing UI and command surface.
- **Distribution:** sideload / APK. No Google Play policy compliance in scope
  for A1 (revisited in A4).
- **End goal:** full parity, delivered incrementally. A1 proves the
  architecture with a single protocol end-to-end.

## A1 scope (MVP)

**In scope:**
- `vpn-engine` compiles as a `cdylib` for Android ABIs via the NDK.
- New `platform/android.rs`: instead of opening `/dev/net/tun`, the engine
  accepts an externally-provided TUN fd and skips OS-level route/IP setup
  (that is done on the Kotlin side via `VpnService.Builder`).
- A Tauri plugin (Kotlin) that:
  - subclasses `android.net.VpnService`,
  - triggers the system VPN consent intent,
  - builds the tunnel (addresses, routes, DNS, MTU) from session params,
  - runs as a **foreground service** with a persistent notification,
  - calls `protect()` on the engine's outbound socket to prevent a routing
    loop,
  - passes the TUN fd into the engine over JNI and drives connect/disconnect.
- **One** protocol working end-to-end (AnyConnect **or** Checkpoint — chosen at
  plan time), with real traffic flowing through the tunnel.
- Connect / disconnect from the Tauri UI on Android.

**Out of scope for A1 (later sub-projects):**
- The second protocol (A2).
- Reconnect/supervision hardening for Doze / screen-off (A2).
- Profiles DB + profile UI on mobile (A3).
- Rich persistent notification, quick-settings tile (A3).
- Always-on VPN, kill-switch, multi-ABI release, APK signing pipeline (A4).

## Architecture (A1)

```
┌─────────────────────────────┐
│  Tauri app (Android APK)     │
│  ┌───────────────────────┐   │
│  │ React UI (WebView)     │   │  invoke() connect/disconnect
│  └──────────┬────────────┘   │
│             │ Tauri command   │
│  ┌──────────▼────────────┐   │
│  │ Tauri VPN plugin (Kotlin)  │  hosts VpnService + consent + FGS
│  │  - VpnService subclass │   │
│  │  - VpnService.Builder  │──── configures routes/DNS/MTU, gets TUN fd
│  │  - protect(socket)     │   │
│  └──────────┬────────────┘   │
│             │ JNI (fd + config, control)
│  ┌──────────▼────────────┐   │
│  │ vpn-engine (.so)       │   │  platform/android.rs uses the passed fd
│  │  protocols, framer,    │   │  (no /dev/net/tun, no route commands)
│  │  cipher, supervision   │   │
│  └───────────────────────┘   │
└─────────────────────────────┘
```

### Component boundaries

1. **`vpn-engine` (`platform/android.rs`)** — new platform module. The engine's
   TUN layer currently relies on the `tun` crate to open + configure the
   interface (Linux ioctl / macOS utun / Windows Wintun). On Android the fd is
   pre-opened by the system, so the TUN layer needs a branch that wraps an
   existing raw fd into the async read/write halves **without** IP assignment or
   route setup. This is the main engine change; protocol clients are untouched.
   - Depends on: an fd (i32) and session params passed from the plugin.
   - Exposes: a JNI-friendly entry point (`run_client_supervised` behind a thin
     `#[no_mangle] extern "C"` / `jni` shim) plus a disconnect signal.

2. **Tauri VPN plugin (Kotlin)** — owns everything Android-specific that can't
   live in Rust:
   - consent (`VpnService.prepare()` → activity result),
   - `VpnService.Builder` construction from session params,
   - foreground service + notification (mandatory for a persistent tunnel),
   - `protect()` on the engine's tunnel socket,
   - JNI calls into the `.so` and callback of state back to the UI.
   - Depends on: the `.so`, session params from the Tauri command.
   - Exposes: Tauri commands `vpn_connect` / `vpn_disconnect` mirroring desktop.

3. **Build pipeline** — `vpn-engine` gains a `cdylib` crate-type for Android
   targets; NDK cross-compile produces the `.so` per ABI, staged into the APK's
   `jniLibs`. `prepare-helper.mjs` is **not** used for Android (no sidecar); a
   separate script/gradle step builds and stages the `.so`.

### Data / control flow (connect)

1. UI calls `vpn_connect(config)` (same command surface as desktop).
2. Plugin calls `VpnService.prepare()`; if consent not yet granted, the system
   dialog is shown. On grant, proceed.
3. Plugin starts the foreground service, builds the tunnel with
   `VpnService.Builder` (addresses/routes/DNS/MTU from config), and obtains the
   TUN fd.
4. Plugin opens the engine's outbound socket path and `protect()`s it (so the
   tunnel's own packets bypass the VPN route).
5. Plugin calls into the `.so` over JNI, handing it the TUN fd + config.
6. Engine runs the chosen protocol client against the fd; state changes
   (Connecting/Established/Reconnecting/Disconnected) are called back to the
   plugin, which emits them to the UI (mirrors the desktop `WireState`).
7. `vpn_disconnect` signals the engine to tear down; plugin stops the FGS.

### Error handling

- Consent denied → surface a clear "VPN permission denied" state to the UI; no
  retry loop.
- `.so` load / JNI failure → fatal, reported as a permanent error (mirrors the
  desktop `ClientMessage::Error { permanent: true }`).
- Engine connection failure → same `WireState`/error surface as desktop; A1 does
  **not** add Android-specific reconnect hardening (that is A2).
- Socket `protect()` failure → abort connect (would otherwise cause a routing
  loop); report as a permanent error.

### Testing

- Engine: unit-test the `platform/android.rs` fd-wrapping path with a socketpair
  fd (no device needed), asserting read/write halves round-trip. Protocol tests
  are unchanged and continue to run on the host.
- Bridge: manual/instrumented test on a real device or emulator with a TUN —
  connect to a test server, confirm traffic flows and disconnect tears down.
  (Automated instrumented tests are a stretch goal, not an A1 gate.)
- The desktop build must remain green: the Android platform module and cdylib
  crate-type are gated to `target_os = "android"` so host builds are unaffected.

## Constraints & risks

- **Background persistence:** Android kills background processes; a VPN must run
  in a foreground service with a persistent notification. In scope for A1 at a
  basic level (a minimal notification), enriched in A3.
- **Routing loop:** the tunnel socket MUST be `protect()`'d or packets loop.
  Non-negotiable, handled in A1.
- **TUN fd ownership:** the fd belongs to the `VpnService`; the engine must not
  close it out from under the service. Lifecycle ownership stays on the Kotlin
  side; the engine borrows the fd for the connection's duration.
- **Emulator limits:** full tunnel testing needs a device/emulator with VPN
  support; behaviour under Doze/screen-off is explicitly deferred to A2.

## Roadmap (post-A1, each its own spec)

- **A2 — Second protocol + connection parity:** enable the remaining protocol;
  reconnect/supervision under Doze/screen-off; DNS/route parity.
- **A3 — App parity:** profiles DB (rusqlite runs on Android) + profile UI on
  mobile; rich persistent notification; quick-settings tile (tray equivalent).
- **A4 — Robustness / release:** always-on VPN, kill-switch, multi-ABI
  (arm64-v8a + x86_64), APK signing + build pipeline.
```
