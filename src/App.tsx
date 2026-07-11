import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type WireState =
  | "Connecting"
  | "Established"
  | "Disconnected"
  | { Reconnecting: { delay_secs: number } };

type ClientMessage =
  | { State: WireState }
  | { Error: { message: string; permanent: boolean } }
  | "Bye";

function stateLabel(s: WireState): string {
  if (typeof s === "string") return s;
  if ("Reconnecting" in s) return `Reconnecting (${s.Reconnecting.delay_secs.toFixed(1)}s)`;
  return "Unknown";
}

export default function App() {
  const [host, setHost] = useState("");
  const [port, setPort] = useState(443);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [protocol, setProtocol] = useState<"AnyConnect" | "Checkpoint">("AnyConnect");
  const [insecure, setInsecure] = useState(false);
  const [cert, setCert] = useState("");
  const [status, setStatus] = useState("Disconnected");
  const [error, setError] = useState("");

  useEffect(() => {
    const un = listen<ClientMessage>("vpn://state", (e) => {
      const msg = e.payload;
      if (typeof msg === "object" && "State" in msg) setStatus(stateLabel(msg.State));
      else if (typeof msg === "object" && "Error" in msg) setError(msg.Error.message);
    });
    return () => { un.then((f) => f()); };
  }, []);

  async function connect() {
    setError("");
    try {
      await invoke("vpn_connect", {
        args: {
          config: {
            host, port, username, protocol,
            cert_sha256: cert.trim() ? cert.trim() : null,
            insecure, verbose: false,
          },
          password,
        },
      });
    } catch (e) { setError(String(e)); }
  }

  async function disconnect() {
    try { await invoke("vpn_disconnect"); } catch (e) { setError(String(e)); }
  }

  return (
    <main className="container">
      <h1>Yellow VPN</h1>
      <p className="status">Status: <b>{status}</b></p>
      {error && <p className="error">{error}</p>}
      <div className="form">
        <input placeholder="Host" value={host} onChange={(e) => setHost(e.target.value)} />
        <input placeholder="Port" type="number" value={port}
               onChange={(e) => setPort(Number(e.target.value))} />
        <input placeholder="Username" value={username}
               onChange={(e) => setUsername(e.target.value)} />
        <input placeholder="Password" type="password" value={password}
               onChange={(e) => setPassword(e.target.value)} />
        <select value={protocol} onChange={(e) => setProtocol(e.target.value as any)}>
          <option value="AnyConnect">AnyConnect (Cisco)</option>
          <option value="Checkpoint">Check Point SNX</option>
        </select>
        <input placeholder="Server cert SHA-256 (optional)" value={cert}
               onChange={(e) => setCert(e.target.value)} />
        <label>
          <input type="checkbox" checked={insecure}
                 onChange={(e) => setInsecure(e.target.checked)} />
          Insecure (skip cert check — danger)
        </label>
      </div>
      <div className="buttons">
        <button onClick={connect}>Connect</button>
        <button onClick={disconnect}>Disconnect</button>
      </div>
      <p className="note">Connecting starts an elevated helper — approve the Windows UAC prompt.</p>
    </main>
  );
}
