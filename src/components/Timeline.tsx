import { useMemo } from "react";
import type { TimelineEvent } from "../lib/types";
import { useChartTokens } from "../lib/chart-theme";

// relative time from the first event, as +m:ss
function rel(base: number, ts: string): string {
  const t = Date.parse(ts);
  if (Number.isNaN(t) || Number.isNaN(base)) return "";
  const secs = Math.max(0, Math.round((t - base) / 1000));
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `+${m}:${String(s).padStart(2, "0")}`;
}

export function Timeline({ events }: { events: TimelineEvent[] }) {
  const tk = useChartTokens();

  // stable model → color map from the shared ramp
  const modelColor = useMemo(() => {
    const models = Array.from(new Set(events.filter((e) => e.model).map((e) => e.model as string)));
    const m = new Map<string, string>();
    models.forEach((mod, i) => m.set(mod, tk.ramp[i % tk.ramp.length]));
    return m;
  }, [events, tk.ramp]);

  const base = events.length ? Date.parse(events[0].ts) : NaN;
  const maxOut = Math.max(1, ...events.map((e) => e.output_tokens ?? 0));

  if (!events.length) {
    return <div className="text-muted text-sm">No events.</div>;
  }

  return (
    <div className="relative pl-6">
      {/* the rail */}
      <div className="timeline-rail absolute left-[7px] top-1 bottom-1 w-[2px] rounded-full opacity-70" />
      <ul className="space-y-2">
        {events.map((e, i) => {
          const isMsg = e.kind === "message";
          const color = isMsg ? modelColor.get(e.model ?? "") ?? tk.ramp[0] : tk.muted;
          // node size: assistant turns scale with output tokens (8 to 18px); tools fixed small
          const size = isMsg ? 8 + 10 * ((e.output_tokens ?? 0) / maxOut) : 7;
          return (
            <li
              key={i}
              className="timeline-node-row relative flex items-center gap-3"
              style={{ animationDelay: `${Math.min(i, 30) * 18}ms` }}
            >
              {/* node */}
              <span
                className="absolute -left-6 grid place-items-center"
                style={{ width: 16, height: 16 }}
              >
                <span
                  style={{
                    width: size,
                    height: size,
                    background: color,
                    borderRadius: isMsg ? "9999px" : "2px",
                    boxShadow: `0 0 10px -1px ${color}`,
                    outline: e.is_mcp ? `2px solid ${tk.ramp[4]}` : "none",
                    outlineOffset: 1,
                  }}
                />
              </span>
              {/* row content */}
              <span className="text-[11px] tabular-nums text-muted w-12 shrink-0">{rel(base, e.ts)}</span>
              <span className="text-sm text-ink truncate">
                {isMsg ? (e.model ?? "assistant") : e.label}
                {e.count > 1 && <span className="ml-1 text-xs text-muted">×{e.count}</span>}
                {e.is_mcp && <span className="ml-1 text-[10px] uppercase tracking-wide text-[color:var(--c5)]">mcp</span>}
              </span>
              {isMsg && e.output_tokens != null && (
                <span className="ml-auto flex items-center gap-2 text-[11px] text-muted shrink-0">
                  <span
                    className="h-1.5 rounded-full"
                    style={{ width: 6 + 60 * ((e.output_tokens ?? 0) / maxOut), background: color, opacity: 0.7 }}
                  />
                  {e.output_tokens} tok
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
