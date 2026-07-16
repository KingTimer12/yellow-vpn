import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { addPluginListener } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { ClientMessage, WireState, stateLabel } from "@/lib/vpn";
import { IS_MOBILE } from "@/hooks/useIsMobile";

export function useVpnState() {
  const [raw, setRaw] = useState<WireState | null>(null);

  useEffect(() => {
    // Mobile: the Kotlin VpnService plugin emits lowercase state strings
    // ("connecting"/"established"/"reconnecting"/"disconnected"/"error:<msg>").
    if (IS_MOBILE) {
      const un = addPluginListener(
        "yellowvpn",
        "state",
        (e: { state: string }) => {
          const s = e.state;
          if (s === "established") {
            setRaw("Established");
            toast.success("Connected", { id: "vpn" });
          } else if (s === "connecting") {
            setRaw("Connecting");
            toast.loading("Connecting…", { id: "vpn" });
          } else if (s === "reconnecting") {
            setRaw({ Reconnecting: { delay_secs: 0 } });
            toast.warning("Reconnecting…", { id: "vpn" });
          } else if (s.startsWith("error:")) {
            setRaw("Disconnected");
            toast.error(s.slice("error:".length) || "Connection error", {
              id: "vpn",
            });
          } else {
            setRaw("Disconnected");
            toast("Disconnected", { id: "vpn" });
          }
        },
      );
      return () => {
        un.then((h) => h.unregister());
      };
    }

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
