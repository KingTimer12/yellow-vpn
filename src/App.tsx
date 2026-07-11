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

export default function App() {
  const { raw } = useVpnState();
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [selected, setSelected] = useState<Profile | null>(null);

  async function refresh() {
    const list = await listProfiles();
    setProfiles(list);
    setSelected((cur) =>
      cur ? list.find((p) => p.id === cur.id) ?? null : null,
    );
  }
  useEffect(() => {
    refresh();
  }, []);

  return (
    <div className="min-h-screen bg-zinc-950 p-6 text-zinc-100">
      <Toaster theme="dark" richColors position="top-center" />
      <h1 className="mb-6 text-2xl font-bold">Yellow VPN</h1>
      <div className="grid gap-6 md:grid-cols-2">
        <StatusHero
          raw={raw}
          activeName={selected?.name ?? null}
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
      </div>
    </div>
  );
}
