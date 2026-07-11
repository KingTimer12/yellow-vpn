import { useState, useEffect } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
  DialogTrigger,
  DialogClose,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { NewProfile, Profile, Protocol } from "@/lib/vpn";

const empty: NewProfile = {
  name: "",
  host: "",
  port: 443,
  username: "",
  password: "",
  protocol: "AnyConnect",
  insecure: false,
  cert_sha256: null,
};

function Label({ children }: { children: React.ReactNode }) {
  return (
    <label className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
      {children}
    </label>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="grid gap-3">
      <div className="flex items-center gap-3">
        <span className="font-mono text-[10px] uppercase tracking-[0.25em] text-muted-foreground">
          {title}
        </span>
        <span className="h-px flex-1 bg-line" />
      </div>
      {children}
    </div>
  );
}

export function ProfileDialog({
  trigger,
  initial,
  onSubmit,
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
    await onSubmit({
      ...f,
      cert_sha256: f.cert_sha256?.trim() ? f.cert_sha256.trim() : null,
    });
    setOpen(false);
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent className="gap-0 overflow-hidden p-0 sm:max-w-lg">
        <div className="h-1 w-full bg-brand" />
        <div className="max-h-[82vh] overflow-y-auto p-6">
          <DialogHeader className="mb-5">
            <p className="font-mono text-[10px] uppercase tracking-[0.3em] text-brand">
              {initial ? "Edit connection" : "New connection"}
            </p>
            <DialogTitle className="text-xl">
              {initial ? initial.name || "Profile" : "Configure profile"}
            </DialogTitle>
          </DialogHeader>

          <div className="grid gap-6">
            <Section title="Gateway">
              <div className="grid gap-1.5">
                <Label>Profile name</Label>
                <Input
                  placeholder="e.g. Work HQ"
                  value={f.name}
                  onChange={(e) => setF({ ...f, name: e.target.value })}
                />
              </div>
              <div className="grid grid-cols-[1fr_auto] gap-3">
                <div className="grid gap-1.5">
                  <Label>Host</Label>
                  <Input
                    className="font-mono"
                    placeholder="vpn.example.com"
                    value={f.host}
                    onChange={(e) => setF({ ...f, host: e.target.value })}
                  />
                </div>
                <div className="grid w-24 gap-1.5">
                  <Label>Port</Label>
                  <Input
                    className="font-mono"
                    type="number"
                    value={f.port}
                    onChange={(e) => setF({ ...f, port: Number(e.target.value) })}
                  />
                </div>
              </div>
            </Section>

            <Section title="Credentials">
              <div className="grid gap-1.5">
                <Label>Username</Label>
                <Input
                  value={f.username}
                  onChange={(e) => setF({ ...f, username: e.target.value })}
                />
              </div>
              <div className="grid gap-1.5">
                <Label>Password</Label>
                <Input
                  type="password"
                  value={f.password}
                  onChange={(e) => setF({ ...f, password: e.target.value })}
                />
              </div>
            </Section>

            <Section title="Protocol & Security">
              <div className="grid gap-1.5">
                <Label>Protocol</Label>
                <Select
                  value={f.protocol}
                  onValueChange={(v) => setF({ ...f, protocol: v as Protocol })}
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="AnyConnect">AnyConnect (Cisco)</SelectItem>
                    <SelectItem value="Checkpoint">Check Point SNX</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-1.5">
                <Label>Server cert SHA-256 (optional)</Label>
                <Input
                  className="font-mono text-xs"
                  placeholder="pin fingerprint…"
                  value={f.cert_sha256 ?? ""}
                  onChange={(e) => setF({ ...f, cert_sha256: e.target.value })}
                />
              </div>
              <div
                className={`flex items-center justify-between rounded-md border px-3 py-2.5 transition-colors ${
                  f.insecure ? "border-destructive/50 bg-destructive/10" : "border-line"
                }`}
              >
                <div>
                  <p className="text-sm font-medium">Skip certificate check</p>
                  <p className="font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
                    Insecure — vulnerable to MITM
                  </p>
                </div>
                <Switch
                  checked={f.insecure}
                  onCheckedChange={(v) => setF({ ...f, insecure: v })}
                />
              </div>
            </Section>
          </div>

          <DialogFooter className="mt-6 gap-2 sm:gap-2">
            <DialogClose asChild>
              <Button variant="ghost">Cancel</Button>
            </DialogClose>
            <Button onClick={save} disabled={!valid} className="font-semibold">
              {initial ? "Save changes" : "Create profile"}
            </Button>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );
}
