import { invoke } from "@tauri-apps/api/core";
import { IS_MOBILE } from "@/hooks/useIsMobile";

export type Protocol = "AnyConnect" | "Checkpoint";

export interface Profile {
  id: number;
  name: string;
  host: string;
  port: number;
  username: string;
  password: string;
  protocol: Protocol;
  insecure: boolean;
  cert_sha256: string | null;
}
export type NewProfile = Omit<Profile, "id">;

export type WireState =
  | "Connecting"
  | "Established"
  | "Disconnected"
  | { Reconnecting: { delay_secs: number } };

export type ClientMessage =
  | { State: WireState }
  | { Error: { message: string; permanent: boolean } }
  | "Bye";

export function stateLabel(s: WireState): string {
  if (typeof s === "string") return s;
  if ("Reconnecting" in s)
    return `Reconnecting (${s.Reconnecting.delay_secs.toFixed(0)}s)`;
  return "Unknown";
}

export async function connectProfile(p: Profile): Promise<void> {
  if (IS_MOBILE) {
    // Android: drive the Kotlin VpnService plugin directly (consent needs an
    // Activity; there is no Rust->Android bridge). See VpnPlugin.kt.
    await invoke("plugin:yellowvpn|connect", {
      host: p.host,
      port: p.port,
      username: p.username,
      password: p.password,
      protocol: p.protocol === "Checkpoint" ? 1 : 0,
      insecure: p.insecure,
      certSha256:
        p.cert_sha256 && p.cert_sha256.trim() ? p.cert_sha256.trim() : "",
    });
    return;
  }
  await invoke("vpn_connect", {
    args: {
      config: {
        host: p.host,
        port: p.port,
        username: p.username,
        protocol: p.protocol,
        cert_sha256:
          p.cert_sha256 && p.cert_sha256.trim() ? p.cert_sha256.trim() : null,
        insecure: p.insecure,
        verbose: false,
      },
      password: p.password,
      profileName: p.name,
    },
  });
}

export const disconnect = () =>
  IS_MOBILE ? invoke("plugin:yellowvpn|disconnect") : invoke("vpn_disconnect");
export const listProfiles = () => invoke<Profile[]>("profiles_list");
export const createProfile = (profile: NewProfile) =>
  invoke<Profile>("profile_create", { profile });
export const updateProfile = (id: number, profile: NewProfile) =>
  invoke<Profile>("profile_update", { id, profile });
export const deleteProfile = (id: number) => invoke("profile_delete", { id });
