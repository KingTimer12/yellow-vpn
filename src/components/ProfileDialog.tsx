import { useState, useEffect } from "react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{initial ? "Edit profile" : "New profile"}</DialogTitle>
        </DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-1">
            <Label>Name</Label>
            <Input
              value={f.name}
              onChange={(e) => setF({ ...f, name: e.target.value })}
            />
          </div>
          <div className="grid gap-1">
            <Label>Host</Label>
            <Input
              value={f.host}
              onChange={(e) => setF({ ...f, host: e.target.value })}
            />
          </div>
          <div className="grid gap-1">
            <Label>Port</Label>
            <Input
              type="number"
              value={f.port}
              onChange={(e) => setF({ ...f, port: Number(e.target.value) })}
            />
          </div>
          <div className="grid gap-1">
            <Label>Username</Label>
            <Input
              value={f.username}
              onChange={(e) => setF({ ...f, username: e.target.value })}
            />
          </div>
          <div className="grid gap-1">
            <Label>Password</Label>
            <Input
              type="password"
              value={f.password}
              onChange={(e) => setF({ ...f, password: e.target.value })}
            />
          </div>
          <div className="grid gap-1">
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
          <div className="grid gap-1">
            <Label>Server cert SHA-256 (optional)</Label>
            <Input
              value={f.cert_sha256 ?? ""}
              onChange={(e) => setF({ ...f, cert_sha256: e.target.value })}
            />
          </div>
          <div className="flex items-center gap-2">
            <Switch
              checked={f.insecure}
              onCheckedChange={(v) => setF({ ...f, insecure: v })}
            />
            <Label>Insecure (skip cert check — danger)</Label>
          </div>
        </div>
        <DialogFooter>
          <Button onClick={save} disabled={!valid}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
