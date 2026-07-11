import { Pencil, Trash2, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
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
    <section className="flex flex-col overflow-hidden rounded-lg border border-line bg-card">
      <div className="flex items-center justify-between border-b border-line px-4 py-3">
        <h2 className="font-mono text-[11px] uppercase tracking-[0.25em] text-muted-foreground">
          Directory
        </h2>
        <ProfileDialog
          trigger={
            <Button size="sm" variant="secondary" className="h-7 gap-1 px-2 text-xs">
              <Plus className="h-3.5 w-3.5" /> New
            </Button>
          }
          onSubmit={onCreate}
        />
      </div>

      {profiles.length === 0 ? (
        <div className="flex flex-col items-center gap-1 px-4 py-12 text-center">
          <p className="text-sm text-muted-foreground">No profiles yet.</p>
          <p className="font-mono text-xs text-muted-foreground/70">
            Add one to start connecting.
          </p>
        </div>
      ) : (
        <ul className="divide-y divide-line">
          {profiles.map((p) => {
            const active = selectedId === p.id;
            return (
              <li key={p.id}>
                <button
                  onClick={() => onSelect(p)}
                  className={`group flex w-full items-center gap-3 px-4 py-3 text-left transition-colors ${
                    active ? "bg-secondary/50" : "hover:bg-secondary/30"
                  }`}
                >
                  <span
                    className={`h-8 w-0.5 rounded-full transition-colors ${
                      active ? "bg-brand" : "bg-transparent"
                    }`}
                  />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{p.name}</p>
                    <p className="truncate font-mono text-xs text-muted-foreground">
                      {p.host}:{p.port}
                    </p>
                  </div>
                  <span className="shrink-0 rounded border border-line px-2 py-0.5 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
                    {p.protocol === "Checkpoint" ? "SNX" : "AnyConnect"}
                  </span>
                  <div
                    className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100"
                    onClick={(e) => e.stopPropagation()}
                  >
                    <ProfileDialog
                      trigger={
                        <Button size="icon" variant="ghost" className="h-7 w-7">
                          <Pencil className="h-3.5 w-3.5" />
                        </Button>
                      }
                      initial={p}
                      onSubmit={(np) => onEdit(p.id, np)}
                    />
                    <Button
                      size="icon"
                      variant="ghost"
                      className="h-7 w-7 text-muted-foreground hover:text-destructive"
                      onClick={() => {
                        if (confirm(`Delete "${p.name}"?`)) onDelete(p.id);
                      }}
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
