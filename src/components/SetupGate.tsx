import { EASE_IN_OUT, useLoop, useReducedMotion } from "@/lib/motion";
import { AlertTriangle, RotateCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { WintunState } from "@/hooks/useWintun";
import iconUrl from "@/assets/yellow_vpn_icon.svg";

const mb = (b: number) => (b / 1048576).toFixed(1);

export function SetupGate({ stage, downloaded, total, error, retry }: WintunState) {
  const reduce = useReducedMotion();
  const pct = total > 0 ? Math.round((downloaded / total) * 100) : 0;
  const indeterminate =
    stage === "checking" || stage === "extracting" || (stage === "downloading" && total === 0);

  const subtitle =
    stage === "checking"
      ? "Checking network driver…"
      : stage === "downloading"
        ? "Downloading network driver"
        : stage === "extracting"
          ? "Installing driver…"
          : "Setup failed";

  // Gentle looping pulse on the logo while work is in progress (not on error).
  const iconRef = useLoop<HTMLImageElement>(
    { scale: [1, 1.06, 1] },
    { duration: 2, ease: EASE_IN_OUT },
    reduce || stage === "error",
  );
  // Indeterminate progress: a bar sweeping across the track, forever.
  const sweepRef = useLoop<HTMLSpanElement>(
    { x: ["-100%", "300%"] },
    { duration: 1.1, ease: EASE_IN_OUT },
    reduce,
  );

  return (
    <div className="flex w-full max-w-sm flex-col items-center gap-6 text-center">
      <img
        ref={iconRef}
        src={iconUrl}
        alt="Yellow VPN"
        className="h-16 w-16 rounded-2xl shadow-lg"
      />

      <div className="space-y-1">
        <h1 className="text-lg font-bold tracking-tight">Preparing Yellow VPN</h1>
        <p
          className={`font-mono text-xs ${
            stage === "error" ? "text-destructive" : "text-muted-foreground"
          }`}
        >
          {subtitle}
        </p>
      </div>

      {stage === "error" ? (
        <div className="flex w-full flex-col items-center gap-3">
          <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-left">
            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" />
            <p className="font-mono text-[11px] leading-relaxed text-muted-foreground">
              {error}
            </p>
          </div>
          <Button onClick={retry} className="gap-2 font-semibold">
            <RotateCw className="h-4 w-4" /> Retry
          </Button>
        </div>
      ) : (
        <div className="w-full space-y-2">
          {/* Progress track */}
          <div className="relative h-2 w-full overflow-hidden rounded-full bg-secondary">
            {indeterminate ? (
              <span
                ref={sweepRef}
                className="absolute inset-y-0 w-1/3 rounded-full bg-brand"
              />
            ) : (
              <span
                className="absolute inset-y-0 left-0 rounded-full bg-brand transition-[width] duration-200 ease-out"
                style={{ width: `${pct}%` }}
              />
            )}
          </div>
          <div className="flex justify-between font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
            <span>{indeterminate ? "please wait" : `${pct}%`}</span>
            {total > 0 && stage === "downloading" && (
              <span>
                {mb(downloaded)} / {mb(total)} MB
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
