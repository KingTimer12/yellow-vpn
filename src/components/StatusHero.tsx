import { WireState } from "@/lib/vpn";
import { Button } from "@/components/ui/button";

function tone(raw: WireState | null): {
  color: string;
  pulse: boolean;
  label: string;
} {
  if (raw === "Established")
    return { color: "bg-emerald-500", pulse: false, label: "Connected" };
  if (raw === "Connecting")
    return { color: "bg-amber-500", pulse: true, label: "Connecting" };
  if (raw && typeof raw === "object" && "Reconnecting" in raw)
    return { color: "bg-amber-500", pulse: true, label: "Reconnecting" };
  return { color: "bg-zinc-500", pulse: false, label: "Disconnected" };
}

export function StatusHero({
  raw,
  activeName,
  onConnect,
  onDisconnect,
  canConnect,
}: {
  raw: WireState | null;
  activeName: string | null;
  onConnect: () => void;
  onDisconnect: () => void;
  canConnect: boolean;
}) {
  const t = tone(raw);
  const connected = raw === "Established";
  return (
    <div className="flex flex-col items-center gap-4 rounded-xl border border-zinc-800 bg-zinc-900/50 p-8">
      <span
        className={`h-16 w-16 rounded-full ${t.color} ${
          t.pulse ? "animate-pulse" : ""
        }`}
      />
      <div className="text-center">
        <p className="text-lg font-semibold">{t.label}</p>
        <p className="text-sm text-zinc-400">
          {activeName ?? "No profile selected"}
        </p>
      </div>
      {connected ? (
        <Button variant="destructive" onClick={onDisconnect}>
          Disconnect
        </Button>
      ) : (
        <Button onClick={onConnect} disabled={!canConnect}>
          Connect
        </Button>
      )}
    </div>
  );
}
