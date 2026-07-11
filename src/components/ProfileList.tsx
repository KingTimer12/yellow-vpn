import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Profile, NewProfile } from "@/lib/vpn";
import { ProfileDialog } from "./ProfileDialog";

export function ProfileList({
  profiles,
  selectedId,
  onSelect,
  onCreate,
  onEdit,
  onDelete,
}: {
  profiles: Profile[];
  selectedId: number | null;
  onSelect: (p: Profile) => void;
  onCreate: (p: NewProfile) => Promise<void>;
  onEdit: (id: number, p: NewProfile) => Promise<void>;
  onDelete: (id: number) => Promise<void>;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-medium text-zinc-400">Profiles</h2>
        <ProfileDialog
          trigger={
            <Button size="sm" variant="secondary">
              + Add
            </Button>
          }
          onSubmit={onCreate}
        />
      </div>
      {profiles.length === 0 && (
        <p className="text-sm text-zinc-500">No profiles yet — add one.</p>
      )}
      {profiles.map((p) => (
        <Card
          key={p.id}
          onClick={() => onSelect(p)}
          className={`flex cursor-pointer flex-row items-center justify-between p-3 ${
            selectedId === p.id ? "ring-2 ring-emerald-500" : ""
          }`}
        >
          <div>
            <p className="font-medium">{p.name}</p>
            <p className="text-xs text-zinc-400">
              {p.host}:{p.port}
            </p>
          </div>
          <div
            className="flex items-center gap-2"
            onClick={(e) => e.stopPropagation()}
          >
            <Badge variant="outline">{p.protocol}</Badge>
            <ProfileDialog
              trigger={
                <Button size="sm" variant="ghost">
                  Edit
                </Button>
              }
              initial={p}
              onSubmit={(np) => onEdit(p.id, np)}
            />
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                if (confirm(`Delete "${p.name}"?`)) onDelete(p.id);
              }}
            >
              Delete
            </Button>
          </div>
        </Card>
      ))}
    </div>
  );
}
