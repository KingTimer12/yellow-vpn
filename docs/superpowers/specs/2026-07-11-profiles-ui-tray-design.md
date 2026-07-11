# Yellow VPN — Profiles, Polished UI, Tray & Background

**Date:** 2026-07-11
**Status:** Approved design, pre-implementation
**Builds on:** 2026-07-11-tauri-vpn-integration-design.md (workspace + elevated helper + pipe IPC)

## Goal

Add a saved-profile system (SQLite), a polished shadcn/Tailwind UI with live
feedback, a system-tray + run-in-background model, and connect-time
notification + hide-to-tray.

## Locked decisions

- **Password storage: plaintext in SQLite.** User's explicit choice. RISK
  (recorded): anyone who reads `profiles.db` obtains every VPN credential. DPAPI
  encryption can be layered on later without a schema change; out of scope now.
- **Tray icon + menu** (Show / Disconnect / Quit); closing the window hides to
  tray and keeps the app + tunnel alive (Discord-style).
- **DB + profile logic live in the unprivileged GUI process**, not the elevated
  helper. Helper stays credential-agnostic; nothing in the IPC/helper changes.
- **Startup shows a profile list; the user manually selects + connects.**
- **On successful connect: hide window to tray + fire a notification.**
- **Full profile CRUD** (create, edit, delete-with-confirm).

## Stack setup

- **Tailwind v4** — CSS-first: `bun add -D tailwindcss @tailwindcss/vite`; add
  `tailwindcss()` to `vite.config.ts` plugins; replace `src/App.css` content
  with `@import "tailwindcss";` (keep a small globals layer). No `tailwind.config.js`.
- **shadcn (latest)** — Tailwind-v4 + React-19 compatible. `@/*` path alias:
  add to `tsconfig.json` (`compilerOptions.baseUrl:"."`, `paths:{"@/*":["./src/*"]}`)
  and `vite.config.ts` (`resolve.alias` `@` → `/src`). `bunx shadcn@latest init`
  (dark base color). Add components: card, button, input, label, select, dialog,
  switch, badge, sonner.
- Dark theme default.

## Architecture

Only the GUI crate (`src-tauri`) and the frontend (`src/`) change. `vpn-engine`,
`vpn-ipc`, and `vpn-helper` are untouched. The connect path reuses the existing
`vpn_connect(args: {config, password})` command.

### Data layer — `src-tauri/src/profiles.rs` (rusqlite, bundled)

- DB file: `%APPDATA%\yellow-vpn\profiles.db` (create dir + file on first use;
  `PRAGMA journal_mode=WAL`).
- Schema:
  ```sql
  CREATE TABLE IF NOT EXISTS profiles (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    host        TEXT NOT NULL,
    port        INTEGER NOT NULL DEFAULT 443,
    username    TEXT NOT NULL,
    password    TEXT NOT NULL,           -- plaintext (see risk note)
    protocol    TEXT NOT NULL,           -- "AnyConnect" | "Checkpoint"
    insecure    INTEGER NOT NULL DEFAULT 0,
    cert_sha256 TEXT                       -- nullable
  );
  ```
- Rust types: `Profile { id: i64, name, host, port: u16, username, password,
  protocol: String, insecure: bool, cert_sha256: Option<String> }` (serde
  Serialize/Deserialize) and `NewProfile` (no id) for create/update input.
- `Db` wrapper holds a `Mutex<rusqlite::Connection>` in Tauri managed state.
- Tauri commands:
  - `profiles_list() -> Vec<Profile>`
  - `profile_create(profile: NewProfile) -> Profile`
  - `profile_update(id: i64, profile: NewProfile) -> Profile`
  - `profile_delete(id: i64) -> ()`
- Unit tests (in-memory `Connection::open_in_memory()`): create→list round-trip,
  update mutates, delete removes, field mapping incl. `insecure` int↔bool and
  nullable `cert_sha256`.

### Connect wiring (frontend only)

The form is gone from the main flow; the frontend loads the selected `Profile`,
maps it to the existing `WireConfig` (`{host, port, username, protocol,
cert_sha256, insecure, verbose:false}`) and calls `vpn_connect({args:{config,
password: profile.password}})`. `vpn_connect` gains ONE optional addition: a
`profileName: String` field on `ConnectArgs` so the Rust reader can name the
profile in the connect notification (see below). `WireConfig`/helper unchanged.

### Tray + background — `src-tauri/src/lib.rs` (Tauri `tray-icon` feature)

- Build a `TrayIcon` in `run()` with a menu: **Show**, **Disconnect**, **Quit**.
  - Show / left-click → show + focus the main window.
  - Disconnect → send `ClientCommand::Disconnect` on the pipe (reuse
    `vpn_disconnect` logic).
  - Quit → send `ClientCommand::Shutdown`, then `app.exit(0)`.
- `WindowEvent::CloseRequested` → `api.prevent_close()` + `window.hide()`
  (do NOT send Shutdown; tunnel stays up). This REPLACES the current
  CloseRequested handler that sent Shutdown.
- Add `tauri = { features = ["tray-icon"] }`; tray uses the existing app icon.

### Notification + hide-on-connect — `src-tauri/src/lib.rs`

- Add `tauri-plugin-notification`; register in `run()`; capability
  `notification:default` in `capabilities/default.json`.
- `VpnState` gains `current_profile: Option<String>`, set in `vpn_connect` from
  `ConnectArgs.profileName`.
- In the pipe **reader task**, on `WireState::Established`:
  1. Fire a notification: title "Yellow VPN", body "Connected to <profile>".
  2. Hide the main window to the tray (`window.hide()`).
  (Still emits `vpn://state` for the toast, as today.)

### Frontend UI (React + shadcn)

- `src/App.tsx` becomes a dashboard shell; split into focused components under
  `src/components/`:
  - `StatusHero.tsx` — big status indicator (color by state: green Established,
    amber pulsing Connecting/Reconnecting, gray Disconnected), active profile +
    assigned IP, Connect/Disconnect button.
  - `ProfileList.tsx` — Card per profile (name, host, protocol Badge, Connect);
    selection highlight; "Add profile" button; per-profile edit/delete.
  - `ProfileDialog.tsx` — shadcn Dialog form for create/edit (name, host, port,
    username, password, protocol Select, insecure Switch, optional cert Input);
    validates required fields client-side.
  - `useVpnState.ts` — hook: subscribes to `vpn://state`, exposes status +
    raises Sonner toasts per transition (Connecting / Established /
    Reconnecting{delay} / Disconnected / Error).
- Sonner `<Toaster/>` mounted once (dark, richColors).
- The assigned IP: extend nothing server-side for now — the toast/status show
  the state; IP display is best-effort (only if already available), not a new
  IPC field. (Keep scope tight.)

## Testing

- `profiles.rs`: rusqlite in-memory unit tests (CRUD + field mapping).
- Frontend: `bun run build` (tsc) must pass; component logic type-checked.
- Manual E2E: create a profile, restart app, select it, connect → toast +
  notification + hide-to-tray; tray menu Disconnect/Show/Quit; close-to-tray
  keeps tunnel; Quit tears down.

## Out of scope

- DPAPI / credential-manager encryption (deferred; schema-compatible later).
- Auto-connect on startup, profile import/export, multi-window.
- New IPC fields for assigned-IP/DNS surfacing.

## Global constraints (carry over)

- GUI crate depends only on `vpn-ipc` (+ tauri, windows-sys, tokio, serde,
  rusqlite, tauri-plugin-notification) — NOT `vpn-engine`.
- Password never logged; plaintext-at-rest is the accepted storage tradeoff.
- edition 2024 / rust 1.88. Windows-target GUI.
- No changes to `vpn-engine`, `vpn-ipc`, or `vpn-helper`.
