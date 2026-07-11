import { useEffect, useState } from "react";
import { Toaster } from "@/components/ui/sonner";
import { useVpnState } from "@/hooks/useVpnState";
import { StatusHero } from "@/components/StatusHero";
import { ProfileList } from "@/components/ProfileList";
import {
  Profile,
  listProfiles,
  createProfile,
  updateProfile,
  deleteProfile,
  connectProfile,
  disconnect,
} from "@/lib/vpn";
import "./App.css";

function tone(raw: ReturnType<typeof useVpnState>["raw"]) {
  if (raw === "Established") return { dot: "bg-ok", text: "text-ok", label: "ONLINE" };
  if (raw === "Connecting") return { dot: "bg-brand", text: "text-brand", label: "LINKING" };
  if (raw && typeof raw === "object") return { dot: "bg-warn", text: "text-warn", label: "RETRY" };
  return { dot: "bg-muted-foreground", text: "text-muted-foreground", label: "OFFLINE" };
}

export default function App() {
  const { raw } = useVpnState();
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [selected, setSelected] = useState<Profile | null>(null);

  async function refresh() {
    const list = await listProfiles();
    setProfiles(list);
    setSelected((cur) => (cur ? list.find((p) => p.id === cur.id) ?? null : null));
  }
  useEffect(() => {
    refresh();
  }, []);

  const t = tone(raw);

  return (
    <div className="min-h-screen bg-background text-foreground">
      <Toaster theme="dark" richColors position="top-right" />

      {/* Top bar */}
      <header className="flex items-center justify-between border-b border-line px-6 py-4">
        <div className="flex items-baseline gap-2">
          <span className="text-xl font-extrabold tracking-tight text-brand">YELLOW</span>
          <span className="font-mono text-xs uppercase tracking-[0.35em] text-muted-foreground">
            vpn
          </span>
        </div>
        <div className="flex items-center gap-2 font-mono text-[11px]">
          <span className={`h-2 w-2 rounded-full ${t.dot}`} />
          <span className={`uppercase tracking-widest ${t.text}`}>{t.label}</span>
        </div>
      </header>

      {/* Control panel */}
      <main className="mx-auto grid max-w-5xl gap-5 p-6 md:grid-cols-[1.05fr_1fr]">
        <StatusHero
          raw={raw}
          active={selected}
          canConnect={!!selected}
          onConnect={() => selected && connectProfile(selected)}
          onDisconnect={() => disconnect()}
        />
        <ProfileList
          profiles={profiles}
          selectedId={selected?.id ?? null}
          onSelect={setSelected}
          onCreate={async (p) => {
            await createProfile(p);
            await refresh();
          }}
          onEdit={async (id, p) => {
            await updateProfile(id, p);
            await refresh();
          }}
          onDelete={async (id) => {
            await deleteProfile(id);
            await refresh();
          }}
        />
      </main>
    </div>
  );
}
