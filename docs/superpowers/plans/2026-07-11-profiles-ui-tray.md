# Profiles + UI + Tray Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Saved VPN profiles in SQLite, a polished shadcn/Tailwind dark UI with live toast feedback, a system-tray run-in-background model, and connect-time notification + hide-to-tray.

**Architecture:** Only the GUI crate (`src-tauri`) and frontend (`src/`) change. `vpn-engine`/`vpn-ipc`/`vpn-helper` are untouched. Profiles live in the unprivileged GUI (rusqlite, `%APPDATA%\yellow-vpn\profiles.db`); connect reuses the existing `vpn_connect` command.

**Tech Stack:** Tauri 2 (tray-icon + notification plugin), rusqlite (bundled), React 19 + Vite, Tailwind v4, shadcn/ui, Sonner.

## Global Constraints

- GUI crate (`src-tauri`) depends only on `vpn-ipc` (+ tauri, windows-sys, tokio, serde, serde_json, rusqlite, tauri-plugin-notification). NEVER `vpn-engine`.
- Password never logged. Plaintext-at-rest in SQLite is the accepted storage tradeoff (user decision) — do not add extra logging of it.
- edition 2024 / rust-version 1.88. Windows-target GUI.
- Do NOT modify `crates/vpn-engine`, `crates/vpn-ipc`, or `crates/vpn-helper`.
- Package manager is **bun**. Path alias `@/*` → `src/*` in both tsconfig and vite.
- Reuse existing `vpn_connect` / `vpn_disconnect` pipe logic; do not change the IPC wire types.

---

## Task 1: Tailwind v4 + shadcn scaffold

**Files:**
- Modify: `vite.config.ts`, `tsconfig.json`, `src/App.css` (or `src/index.css`), `package.json` (deps)
- Create: `components.json` (shadcn), `src/lib/utils.ts`, `src/components/ui/*` (generated)

**Interfaces:**
- Produces: working Tailwind v4 build, `@/*` alias, and shadcn components (button, card, input, label, select, dialog, switch, badge, sonner) importable from `@/components/ui/*`; `cn()` from `@/lib/utils`.

- [ ] **Step 1: Install Tailwind v4 + Vite plugin**

```bash
cd /d/app/yellow-vpn
bun add -D tailwindcss @tailwindcss/vite
```

- [ ] **Step 2: Add path alias + Tailwind plugin to Vite**

Edit `vite.config.ts` — add the tailwind plugin and the `@` alias. Merge into the existing config (keep the react plugin and the existing `clearScreen`/server settings):

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

export default defineConfig(async () => ({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
}));
```

(If the existing file has Tauri-specific `server`/`envPrefix` settings, preserve them; only add `tailwindcss()` and `resolve.alias`.)

- [ ] **Step 3: Add the alias to tsconfig**

Edit `tsconfig.json` `compilerOptions` — add:

```json
"baseUrl": ".",
"paths": { "@/*": ["./src/*"] }
```

- [ ] **Step 4: Replace CSS entry with Tailwind import**

Overwrite `src/App.css` with:

```css
@import "tailwindcss";
```

Ensure `src/main.tsx` imports it (`import "./App.css";`) — it already imports App.css via App; if not, add the import in `main.tsx`.

- [ ] **Step 5: Init shadcn + add components**

```bash
bunx --bun shadcn@latest init -d -b neutral
bunx --bun shadcn@latest add button card input label select dialog switch badge sonner
```

If `init` prompts despite `-d`, answer: TypeScript yes, style default, base color neutral, CSS file `src/App.css`, CSS variables yes, alias `@/components` and `@/lib/utils`. If shadcn requires a `tailwind.config` for v4 it will create the minimal one; accept it.

- [ ] **Step 6: Smoke-test the build**

Add a temporary shadcn Button to `src/App.tsx` (replace the body's returned JSX with `<Button>Test</Button>` plus its import) ONLY to verify wiring, then run:

Run: `bun run build`
Expected: `tsc && vite build` succeeds; a CSS asset is emitted. Revert the temporary Button edit (App.tsx is fully rewritten in Task 3).

- [ ] **Step 7: Commit**

```bash
git add package.json bun.lock vite.config.ts tsconfig.json components.json src/lib src/components/ui src/App.css
git commit -m "build: add Tailwind v4 + shadcn/ui scaffold"
```

---

## Task 2: rusqlite profiles module + Tauri commands

**Files:**
- Create: `src-tauri/src/profiles.rs`
- Modify: `src-tauri/Cargo.toml` (add rusqlite), `src-tauri/src/lib.rs` (mod + managed state + handler registration)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: `profiles::{Profile, NewProfile, Db}`; Tauri commands `profiles_list`, `profile_create`, `profile_update`, `profile_delete`. `Profile` serde shape: `{ id: i64, name: string, host: string, port: number, username: string, password: string, protocol: string, insecure: boolean, cert_sha256: string|null }`. `NewProfile` = same minus `id`.

- [ ] **Step 1: Add rusqlite dependency**

Edit `src-tauri/Cargo.toml` `[dependencies]`:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```

- [ ] **Step 2: Write the failing tests (module skeleton + tests first)**

Create `src-tauri/src/profiles.rs`:

```rust
//! Saved VPN connection profiles, persisted in a local SQLite DB
//! (%APPDATA%\yellow-vpn\profiles.db). Lives entirely in the unprivileged GUI
//! process; the elevated helper never sees this DB. Passwords are stored in
//! plaintext per the product decision (see the design's risk note).
use std::sync::Mutex;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub protocol: String,
    pub insecure: bool,
    pub cert_sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewProfile {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub protocol: String,
    pub insecure: bool,
    pub cert_sha256: Option<String>,
}

/// Managed-state wrapper around the SQLite connection.
pub struct Db(pub Mutex<Connection>);

impl Db {
    /// Open (or create) the DB at `path` and ensure the schema exists.
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Db(Mutex::new(conn)))
    }

    fn init(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS profiles (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                host TEXT NOT NULL,
                port INTEGER NOT NULL DEFAULT 443,
                username TEXT NOT NULL,
                password TEXT NOT NULL,
                protocol TEXT NOT NULL,
                insecure INTEGER NOT NULL DEFAULT 0,
                cert_sha256 TEXT
             );",
        )?;
        Ok(())
    }

    pub fn list(&self) -> rusqlite::Result<Vec<Profile>> {
        let conn = self.0.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id,name,host,port,username,password,protocol,insecure,cert_sha256
             FROM profiles ORDER BY name",
        )?;
        let rows = stmt.query_map([], Self::row_to_profile)?;
        rows.collect()
    }

    pub fn create(&self, p: &NewProfile) -> rusqlite::Result<Profile> {
        let conn = self.0.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO profiles (name,host,port,username,password,protocol,insecure,cert_sha256)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![p.name, p.host, p.port, p.username, p.password, p.protocol, p.insecure as i64, p.cert_sha256],
        )?;
        let id = conn.last_insert_rowid();
        Self::get(&conn, id)
    }

    pub fn update(&self, id: i64, p: &NewProfile) -> rusqlite::Result<Profile> {
        let conn = self.0.lock().expect("db mutex poisoned");
        conn.execute(
            "UPDATE profiles SET name=?1,host=?2,port=?3,username=?4,password=?5,protocol=?6,insecure=?7,cert_sha256=?8
             WHERE id=?9",
            rusqlite::params![p.name, p.host, p.port, p.username, p.password, p.protocol, p.insecure as i64, p.cert_sha256, id],
        )?;
        Self::get(&conn, id)
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.0.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM profiles WHERE id=?1", [id])?;
        Ok(())
    }

    fn get(conn: &Connection, id: i64) -> rusqlite::Result<Profile> {
        conn.query_row(
            "SELECT id,name,host,port,username,password,protocol,insecure,cert_sha256
             FROM profiles WHERE id=?1",
            [id],
            Self::row_to_profile,
        )
    }

    fn row_to_profile(row: &rusqlite::Row) -> rusqlite::Result<Profile> {
        Ok(Profile {
            id: row.get(0)?,
            name: row.get(1)?,
            host: row.get(2)?,
            port: row.get(3)?,
            username: row.get(4)?,
            password: row.get(5)?,
            protocol: row.get(6)?,
            insecure: row.get::<_, i64>(7)? != 0,
            cert_sha256: row.get(8)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> Db {
        let conn = Connection::open_in_memory().unwrap();
        Db::init(&conn).unwrap();
        Db(Mutex::new(conn))
    }

    fn sample() -> NewProfile {
        NewProfile {
            name: "work".into(), host: "vpn.example.com".into(), port: 443,
            username: "alice".into(), password: "s3cret".into(),
            protocol: "Checkpoint".into(), insecure: true, cert_sha256: None,
        }
    }

    #[test]
    fn create_then_list_round_trips() {
        let db = mem_db();
        let created = db.create(&sample()).unwrap();
        assert!(created.id > 0);
        let all = db.list().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "work");
        assert!(all[0].insecure); // int 1 -> bool true
        assert_eq!(all[0].cert_sha256, None);
        assert_eq!(all[0].port, 443);
    }

    #[test]
    fn update_mutates() {
        let db = mem_db();
        let c = db.create(&sample()).unwrap();
        let mut np = sample();
        np.name = "work-edited".into();
        np.insecure = false;
        np.cert_sha256 = Some("aa:bb".into());
        let u = db.update(c.id, &np).unwrap();
        assert_eq!(u.name, "work-edited");
        assert!(!u.insecure);
        assert_eq!(u.cert_sha256, Some("aa:bb".into()));
    }

    #[test]
    fn delete_removes() {
        let db = mem_db();
        let c = db.create(&sample()).unwrap();
        db.delete(c.id).unwrap();
        assert!(db.list().unwrap().is_empty());
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p yellow-vpn profiles`
Expected: 3 tests pass (`create_then_list_round_trips`, `update_mutates`, `delete_removes`).

- [ ] **Step 4: Add the Tauri commands + register**

In `src-tauri/src/lib.rs`, add `mod profiles;` and the command wrappers (place near the other commands):

```rust
use profiles::{Db, NewProfile, Profile};

fn db_path() -> std::path::PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    let dir = std::path::Path::new(&base).join("yellow-vpn");
    let _ = std::fs::create_dir_all(&dir);
    dir.join("profiles.db")
}

#[tauri::command]
async fn profiles_list(db: tauri::State<'_, Db>) -> Result<Vec<Profile>, String> {
    db.list().map_err(|e| e.to_string())
}

#[tauri::command]
async fn profile_create(db: tauri::State<'_, Db>, profile: NewProfile) -> Result<Profile, String> {
    db.create(&profile).map_err(|e| e.to_string())
}

#[tauri::command]
async fn profile_update(db: tauri::State<'_, Db>, id: i64, profile: NewProfile) -> Result<Profile, String> {
    db.update(id, &profile).map_err(|e| e.to_string())
}

#[tauri::command]
async fn profile_delete(db: tauri::State<'_, Db>, id: i64) -> Result<(), String> {
    db.delete(id).map_err(|e| e.to_string())
}
```

In `run()`, before `.invoke_handler`, add the managed DB:

```rust
.manage(Db::open(&db_path()).expect("failed to open profiles.db"))
```

and extend `tauri::generate_handler![...]` to include `profiles_list, profile_create, profile_update, profile_delete` alongside the existing `vpn_connect, vpn_disconnect, vpn_status`.

- [ ] **Step 5: Build**

Run: `cargo build -p yellow-vpn`
Expected: compiles. (rusqlite `bundled` compiles SQLite from source on first build — may take a minute.)

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/profiles.rs src-tauri/src/lib.rs Cargo.lock
git commit -m "feat: sqlite profiles module + CRUD commands"
```

---

## Task 3: Frontend state hook + toasts + dashboard shell

**Files:**
- Create: `src/lib/vpn.ts` (types + helpers), `src/hooks/useVpnState.ts`, `src/components/StatusHero.tsx`
- Modify: `src/App.tsx` (dashboard shell + Toaster)

**Interfaces:**
- Consumes: `vpn_connect`, `vpn_disconnect` commands; `vpn://state` events; shadcn ui from Task 1.
- Produces: `useVpnState()` hook returning `{ status: string, raw: WireState|null }`; `StatusHero` component; typed `WireConfig`/`WireState`/`ClientMessage`/`Profile` TS types in `src/lib/vpn.ts`.

- [ ] **Step 1: Shared TS types + connect helper**

Create `src/lib/vpn.ts`:

```ts
import { invoke } from "@tauri-apps/api/core";

export type Protocol = "AnyConnect" | "Checkpoint";

export interface Profile {
  id: number; name: string; host: string; port: number;
  username: string; password: string; protocol: Protocol;
  insecure: boolean; cert_sha256: string | null;
}
export type NewProfile = Omit<Profile, "id">;

export type WireState =
  | "Connecting" | "Established" | "Disconnected"
  | { Reconnecting: { delay_secs: number } };

export type ClientMessage =
  | { State: WireState }
  | { Error: { message: string; permanent: boolean } }
  | "Bye";

export function stateLabel(s: WireState): string {
  if (typeof s === "string") return s;
  if ("Reconnecting" in s) return `Reconnecting (${s.Reconnecting.delay_secs.toFixed(0)}s)`;
  return "Unknown";
}

export async function connectProfile(p: Profile): Promise<void> {
  await invoke("vpn_connect", {
    args: {
      config: {
        host: p.host, port: p.port, username: p.username, protocol: p.protocol,
        cert_sha256: p.cert_sha256 && p.cert_sha256.trim() ? p.cert_sha256.trim() : null,
        insecure: p.insecure, verbose: false,
      },
      password: p.password,
      profileName: p.name,
    },
  });
}

export const disconnect = () => invoke("vpn_disconnect");
export const listProfiles = () => invoke<Profile[]>("profiles_list");
export const createProfile = (profile: NewProfile) => invoke<Profile>("profile_create", { profile });
export const updateProfile = (id: number, profile: NewProfile) => invoke<Profile>("profile_update", { id, profile });
export const deleteProfile = (id: number) => invoke("profile_delete", { id });
```

- [ ] **Step 2: The vpn-state hook with Sonner feedback**

Create `src/hooks/useVpnState.ts`:

```ts
import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { ClientMessage, WireState, stateLabel } from "@/lib/vpn";

export function useVpnState() {
  const [raw, setRaw] = useState<WireState | null>(null);

  useEffect(() => {
    const un = listen<ClientMessage>("vpn://state", (e) => {
      const msg = e.payload;
      if (typeof msg === "object" && "State" in msg) {
        const s = msg.State;
        setRaw(s);
        if (s === "Established") toast.success("Connected");
        else if (s === "Connecting") toast.loading("Connecting…", { id: "vpn" });
        else if (s === "Disconnected") toast("Disconnected", { id: "vpn" });
        else if (typeof s === "object" && "Reconnecting" in s)
          toast.warning(stateLabel(s), { id: "vpn" });
      } else if (typeof msg === "object" && "Error" in msg) {
        toast.error(msg.Error.message);
      }
    });
    return () => { un.then((f) => f()); };
  }, []);

  const status = raw ? stateLabel(raw) : "Disconnected";
  return { status, raw };
}
```

- [ ] **Step 3: StatusHero component**

Create `src/components/StatusHero.tsx`:

```tsx
import { WireState } from "@/lib/vpn";
import { Button } from "@/components/ui/button";

function tone(raw: WireState | null): { color: string; pulse: boolean; label: string } {
  if (raw === "Established") return { color: "bg-emerald-500", pulse: false, label: "Connected" };
  if (raw === "Connecting") return { color: "bg-amber-500", pulse: true, label: "Connecting" };
  if (raw && typeof raw === "object" && "Reconnecting" in raw)
    return { color: "bg-amber-500", pulse: true, label: "Reconnecting" };
  return { color: "bg-zinc-500", pulse: false, label: "Disconnected" };
}

export function StatusHero({
  raw, activeName, onConnect, onDisconnect, canConnect,
}: {
  raw: WireState | null; activeName: string | null;
  onConnect: () => void; onDisconnect: () => void; canConnect: boolean;
}) {
  const t = tone(raw);
  const connected = raw === "Established";
  return (
    <div className="flex flex-col items-center gap-4 rounded-xl border border-zinc-800 bg-zinc-900/50 p-8">
      <span className={`h-16 w-16 rounded-full ${t.color} ${t.pulse ? "animate-pulse" : ""}`} />
      <div className="text-center">
        <p className="text-lg font-semibold">{t.label}</p>
        <p className="text-sm text-zinc-400">{activeName ?? "No profile selected"}</p>
      </div>
      {connected ? (
        <Button variant="destructive" onClick={onDisconnect}>Disconnect</Button>
      ) : (
        <Button onClick={onConnect} disabled={!canConnect}>Connect</Button>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Dashboard shell in App.tsx (temporary profile stub)**

Rewrite `src/App.tsx` to mount the Toaster + StatusHero, driven by `useVpnState`. Profiles come in Task 4 — for now keep a placeholder `activeName={null}` and a disabled Connect, plus the `<Toaster/>`:

```tsx
import { Toaster } from "@/components/ui/sonner";
import { useVpnState } from "@/hooks/useVpnState";
import { StatusHero } from "@/components/StatusHero";
import { disconnect } from "@/lib/vpn";
import "./App.css";

export default function App() {
  const { raw } = useVpnState();
  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-100 p-6">
      <Toaster theme="dark" richColors position="top-center" />
      <h1 className="mb-6 text-2xl font-bold">Yellow VPN</h1>
      <StatusHero raw={raw} activeName={null} canConnect={false}
        onConnect={() => {}} onDisconnect={() => disconnect()} />
    </div>
  );
}
```

- [ ] **Step 5: Type-check + build**

Run: `bun run build`
Expected: passes, no type errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/vpn.ts src/hooks/useVpnState.ts src/components/StatusHero.tsx src/App.tsx
git commit -m "feat: vpn-state hook + toasts + status hero"
```

---

## Task 4: Profile list + create/edit/delete dialog

**Files:**
- Create: `src/components/ProfileList.tsx`, `src/components/ProfileDialog.tsx`
- Modify: `src/App.tsx` (wire profiles + selection + connect)

**Interfaces:**
- Consumes: `listProfiles/createProfile/updateProfile/deleteProfile/connectProfile` from `@/lib/vpn`; shadcn Dialog/Card/Input/Select/Switch/Badge.
- Produces: full profile CRUD UI; App selects a profile and connects it.

- [ ] **Step 1: ProfileDialog (create/edit form)**

Create `src/components/ProfileDialog.tsx`:

```tsx
import { useState, useEffect } from "react";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter, DialogTrigger } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { NewProfile, Profile, Protocol } from "@/lib/vpn";

const empty: NewProfile = {
  name: "", host: "", port: 443, username: "", password: "",
  protocol: "AnyConnect", insecure: false, cert_sha256: null,
};

export function ProfileDialog({
  trigger, initial, onSubmit,
}: {
  trigger: React.ReactNode;
  initial?: Profile;
  onSubmit: (p: NewProfile) => Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [f, setF] = useState<NewProfile>(empty);

  useEffect(() => {
    if (open) setF(initial ? { ...initial } : empty);
  }, [open, initial]);

  const valid = f.name && f.host && f.username;

  async function save() {
    await onSubmit({ ...f, cert_sha256: f.cert_sha256?.trim() ? f.cert_sha256.trim() : null });
    setOpen(false);
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader><DialogTitle>{initial ? "Edit profile" : "New profile"}</DialogTitle></DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-1"><Label>Name</Label><Input value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} /></div>
          <div className="grid gap-1"><Label>Host</Label><Input value={f.host} onChange={(e) => setF({ ...f, host: e.target.value })} /></div>
          <div className="grid gap-1"><Label>Port</Label><Input type="number" value={f.port} onChange={(e) => setF({ ...f, port: Number(e.target.value) })} /></div>
          <div className="grid gap-1"><Label>Username</Label><Input value={f.username} onChange={(e) => setF({ ...f, username: e.target.value })} /></div>
          <div className="grid gap-1"><Label>Password</Label><Input type="password" value={f.password} onChange={(e) => setF({ ...f, password: e.target.value })} /></div>
          <div className="grid gap-1"><Label>Protocol</Label>
            <Select value={f.protocol} onValueChange={(v) => setF({ ...f, protocol: v as Protocol })}>
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="AnyConnect">AnyConnect (Cisco)</SelectItem>
                <SelectItem value="Checkpoint">Check Point SNX</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="grid gap-1"><Label>Server cert SHA-256 (optional)</Label><Input value={f.cert_sha256 ?? ""} onChange={(e) => setF({ ...f, cert_sha256: e.target.value })} /></div>
          <div className="flex items-center gap-2"><Switch checked={f.insecure} onCheckedChange={(v) => setF({ ...f, insecure: v })} /><Label>Insecure (skip cert check — danger)</Label></div>
        </div>
        <DialogFooter><Button onClick={save} disabled={!valid}>Save</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
```

- [ ] **Step 2: ProfileList**

Create `src/components/ProfileList.tsx`:

```tsx
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Profile, NewProfile } from "@/lib/vpn";
import { ProfileDialog } from "./ProfileDialog";

export function ProfileList({
  profiles, selectedId, onSelect, onCreate, onEdit, onDelete,
}: {
  profiles: Profile[]; selectedId: number | null;
  onSelect: (p: Profile) => void;
  onCreate: (p: NewProfile) => Promise<void>;
  onEdit: (id: number, p: NewProfile) => Promise<void>;
  onDelete: (id: number) => Promise<void>;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium text-zinc-400">Profiles</h2>
        <ProfileDialog trigger={<Button size="sm" variant="secondary">+ Add</Button>} onSubmit={onCreate} />
      </div>
      {profiles.length === 0 && <p className="text-sm text-zinc-500">No profiles yet — add one.</p>}
      {profiles.map((p) => (
        <Card key={p.id}
          onClick={() => onSelect(p)}
          className={`flex cursor-pointer items-center justify-between p-3 ${selectedId === p.id ? "ring-2 ring-emerald-500" : ""}`}>
          <div>
            <p className="font-medium">{p.name}</p>
            <p className="text-xs text-zinc-400">{p.host}:{p.port}</p>
          </div>
          <div className="flex items-center gap-2" onClick={(e) => e.stopPropagation()}>
            <Badge variant="outline">{p.protocol}</Badge>
            <ProfileDialog trigger={<Button size="sm" variant="ghost">Edit</Button>} initial={p} onSubmit={(np) => onEdit(p.id, np)} />
            <Button size="sm" variant="ghost" onClick={() => { if (confirm(`Delete "${p.name}"?`)) onDelete(p.id); }}>Delete</Button>
          </div>
        </Card>
      ))}
    </div>
  );
}
```

- [ ] **Step 3: Wire into App.tsx**

Rewrite `src/App.tsx` to load profiles, manage selection, and connect the selected one:

```tsx
import { useEffect, useState } from "react";
import { Toaster } from "@/components/ui/sonner";
import { useVpnState } from "@/hooks/useVpnState";
import { StatusHero } from "@/components/StatusHero";
import { ProfileList } from "@/components/ProfileList";
import {
  Profile, NewProfile, listProfiles, createProfile, updateProfile,
  deleteProfile, connectProfile, disconnect,
} from "@/lib/vpn";

export default function App() {
  const { raw } = useVpnState();
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [selected, setSelected] = useState<Profile | null>(null);

  async function refresh() {
    const list = await listProfiles();
    setProfiles(list);
    setSelected((cur) => cur ? list.find((p) => p.id === cur.id) ?? null : null);
  }
  useEffect(() => { refresh(); }, []);

  return (
    <div className="min-h-screen bg-zinc-950 text-zinc-100 p-6">
      <Toaster theme="dark" richColors position="top-center" />
      <h1 className="mb-6 text-2xl font-bold">Yellow VPN</h1>
      <div className="grid gap-6 md:grid-cols-2">
        <StatusHero raw={raw} activeName={selected?.name ?? null}
          canConnect={!!selected}
          onConnect={() => selected && connectProfile(selected)}
          onDisconnect={() => disconnect()} />
        <ProfileList profiles={profiles} selectedId={selected?.id ?? null}
          onSelect={setSelected}
          onCreate={async (p) => { await createProfile(p); await refresh(); }}
          onEdit={async (id, p) => { await updateProfile(id, p); await refresh(); }}
          onDelete={async (id) => { await deleteProfile(id); await refresh(); }} />
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Build**

Run: `bun run build`
Expected: passes, no type errors.

- [ ] **Step 5: Commit**

```bash
git add src/components/ProfileList.tsx src/components/ProfileDialog.tsx src/App.tsx
git commit -m "feat: profile list + create/edit/delete UI wired to connect"
```

---

## Task 5: System tray + close-to-background

**Files:**
- Modify: `src-tauri/Cargo.toml` (tauri tray-icon feature), `src-tauri/src/lib.rs`, `src-tauri/tauri.conf.json` (window: no auto-exit)

**Interfaces:**
- Consumes: existing `Shared` VPN state + `pipe::send_command`.
- Produces: tray icon with Show/Disconnect/Quit; CloseRequested hides to tray instead of exiting; only Quit tears down + exits.

- [ ] **Step 1: Enable the tray-icon feature**

Edit `src-tauri/Cargo.toml`:

```toml
tauri = { version = "2", features = ["tray-icon"] }
```

- [ ] **Step 2: Build the tray + rework window-close in `run()`**

In `src-tauri/src/lib.rs`, inside `tauri::Builder`, replace the existing `on_window_event` CloseRequested handler (which sent Shutdown) with hide-to-tray, and add the tray in `.setup`:

```rust
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::Manager;

// inside Builder chain:
.setup(|app| {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let disconnect = MenuItem::with_id(app, "disconnect", "Disconnect", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &disconnect, &quit])?;
    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => { if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); } }
            "disconnect" => {
                let shared = app.state::<Shared>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    let mut st = shared.lock().await;
                    if let Some(mut w) = st.writer.take() {
                        let _ = pipe::send_command(&mut w, &ClientCommand::Disconnect).await;
                        st.writer = Some(w);
                    }
                });
            }
            "quit" => {
                let shared = app.state::<Shared>().inner().clone();
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let mut st = shared.lock().await;
                    if let Some(mut w) = st.writer.take() {
                        let _ = pipe::send_command(&mut w, &ClientCommand::Shutdown).await;
                    }
                    handle.exit(0);
                });
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            use tauri::tray::{TrayIconEvent, MouseButton, MouseButtonState};
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                if let Some(w) = tray.app_handle().get_webview_window("main") { let _ = w.show(); let _ = w.set_focus(); }
            }
        })
        .build(app)?;
    Ok(())
})
.on_window_event(|window, event| {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        // Hide to tray instead of exiting; keep the tunnel + helper alive.
        api.prevent_close();
        let _ = window.hide();
    }
})
```

Remove the old CloseRequested body that sent `Shutdown`. Keep `mod pipe;` and the `ClientCommand` import.

- [ ] **Step 3: Build**

Run: `cargo build -p yellow-vpn`
Expected: compiles. If `default_window_icon()` returns `None` in dev, the `.unwrap()` is acceptable (icons are configured in tauri.conf.json); if it panics in dev, guard with `if let Some(icon) = app.default_window_icon() { builder.icon(icon.clone()) }`.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/lib.rs
git commit -m "feat: system tray + close-to-background (hide instead of exit)"
```

---

## Task 6: Notification + hide-on-connect

**Files:**
- Modify: `src-tauri/Cargo.toml` (notification plugin), `src-tauri/src/lib.rs`, `src-tauri/capabilities/default.json` (notification permission), `package.json` (JS plugin — only if the frontend triggers; here Rust-side, so JS dep optional)

**Interfaces:**
- Consumes: `ConnectArgs` (gains `profile_name`), the pipe reader task, `Shared`.
- Produces: on `Established`, a desktop notification + main window hidden to tray.

- [ ] **Step 1: Add the notification plugin**

Edit `src-tauri/Cargo.toml`:

```toml
tauri-plugin-notification = "2"
```

- [ ] **Step 2: Register plugin + capability**

In `run()` builder chain add `.plugin(tauri_plugin_notification::init())`.

Edit `src-tauri/capabilities/default.json` — add `"notification:default"` to the `permissions` array.

- [ ] **Step 3: Carry the profile name + notify/hide on Established**

Add `profile_name` to `ConnectArgs` and a `current_profile` field to `VpnState`:

```rust
#[derive(Deserialize)]
struct ConnectArgs {
    config: WireConfig,
    password: String,
    #[serde(rename = "profileName")]
    profile_name: String,
}
```

In `VpnState` add `current_profile: Option<String>` (update the hand-written `Default` to `None`). In `vpn_connect`, after building the connection, set `state.lock().await.current_profile = Some(args.profile_name.clone());` (do this where the writer is stored). Pass an `AppHandle` + the profile name into the reader task and, on `WireState::Established`, notify + hide:

```rust
// inside the reader task, when a State message arrives:
if let ClientMessage::State(s) = &msg {
    shared2.lock().await.status = s.clone();
    if matches!(s, WireState::Established) {
        use tauri_plugin_notification::NotificationExt;
        let name = shared2.lock().await.current_profile.clone().unwrap_or_default();
        let _ = app2.notification().builder()
            .title("Yellow VPN")
            .body(format!("Connected to {name}"))
            .show();
        if let Some(w) = app2.get_webview_window("main") { let _ = w.hide(); }
    }
}
```

(`app2` is the `AppHandle` already cloned into the reader task in the existing code; `get_webview_window` needs `use tauri::Manager;` which Task 5 added.)

- [ ] **Step 4: Build**

Run: `cargo build -p yellow-vpn`
Expected: compiles.

- [ ] **Step 5: Frontend passes profileName**

Confirm `src/lib/vpn.ts` `connectProfile` already sends `profileName: p.name` (it does, from Task 3). No change needed; if the Rust field rename differs, align them. Run `bun run build` to confirm no breakage.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/lib.rs src-tauri/capabilities/default.json Cargo.lock
git commit -m "feat: connect notification + hide-to-tray on Established"
```

---

## Task 7: Manual E2E verification

**Files:** none.

- [ ] **Step 1: Build everything**

Run: `cargo build --workspace` and `bun run build`. Then `bun run predev:helper` (build + stage helper + wintun.dll). Expected: all clean.

- [ ] **Step 2: Dev run**

Run: `bun run tauri dev`. Expected: dark dashboard, empty profile list.

- [ ] **Step 3: Profiles**

Add a profile (real gateway creds), confirm it appears. Restart the app (`bun run tauri dev` again) → the profile persists (loaded from `%APPDATA%\yellow-vpn\profiles.db`). Edit it, delete a throwaway one.

- [ ] **Step 4: Connect flow**

Select the profile, Connect → approve UAC. Expected: toasts Connecting → Connected; a desktop **notification** "Connected to <name>"; the **window hides to tray**. Tray left-click restores it; status hero shows green Connected.

- [ ] **Step 5: Tray + background**

Close the window (X) → app keeps running in tray, tunnel stays up (verify traffic). Tray → Disconnect → toast Disconnected. Tray → Quit → helper tears down (check `%LOCALAPPDATA%\yellow-vpn\helper.log` routes-before-TUN) and app exits; no orphaned routes (`route print`).

- [ ] **Step 6: Commit any fixes**

```bash
git add -A && git commit -m "fix: address issues found in profiles/UI/tray e2e"
```

---

## Self-Review notes

- **Spec coverage:** Tailwind+shadcn (T1), SQLite profiles + CRUD commands (T2), UI feedback via toasts + status hero (T3), profile CRUD UI + connect wiring (T4), tray + close-to-background (T5), notification + hide-on-connect (T6), manual E2E (T7). All spec sections mapped.
- **No IPC/helper/engine changes** — every task touches only `src-tauri` + `src/` (plus deps). `vpn_connect` reused; only `ConnectArgs` gains a `profileName` field (additive, frontend already sends it).
- **Type consistency:** `Profile`/`NewProfile` fields identical across `profiles.rs` (T2) and `src/lib/vpn.ts` (T3); `WireConfig` shape sent by `connectProfile` matches the existing Rust `WireConfig`; `WireState`/`ClientMessage` TS types match `vpn-ipc`.
- **Plaintext password** is an explicit, recorded product decision — not flagged as a defect by reviewers, but the risk note stays in the spec.
