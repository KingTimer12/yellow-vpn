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
        if (s === "Established") toast.success("Connected", { id: "vpn" });
        else if (s === "Connecting") toast.loading("Connecting…", { id: "vpn" });
        else if (s === "Disconnected") toast("Disconnected", { id: "vpn" });
        else if (typeof s === "object" && "Reconnecting" in s)
          toast.warning(stateLabel(s), { id: "vpn" });
      } else if (typeof msg === "object" && "Error" in msg) {
        toast.error(msg.Error.message);
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const status = raw ? stateLabel(raw) : "Disconnected";
  return { status, raw };
}
