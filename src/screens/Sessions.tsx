import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getSessions } from "../lib/ipc";
import { useFilters } from "../state/filters";
import { useNav } from "../state/nav";
import { ChartCard } from "../components/ChartCard";
import { usd, compact } from "../lib/format";
import type { SessionSummary } from "../lib/types";

type SortKey = "cost_usd" | "tokens" | "duration_secs" | "started_at";

function dur(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}
function shortId(id: string) { return id.length > 10 ? id.slice(0, 8) : id; }
function startedLabel(s: string | null) { return s ? s.slice(0, 16).replace("T", " ") : "-"; }

export default function Sessions() {
  const { filters } = useFilters();
  const { sessionsProject, openSession, setScreen } = useNav();
  const [sort, setSort] = useState<SortKey>("started_at");

  // When arriving from a Projects row, override the project filter.
  const effective = sessionsProject ? { ...filters, project: sessionsProject } : filters;
  const q = useQuery({ queryKey: ["sessions", effective], queryFn: () => getSessions(effective) });

  const rows = useMemo(() => {
    const data = [...(q.data ?? [])];
    data.sort((a, b) => {
      if (sort === "started_at") return String(b.started_at).localeCompare(String(a.started_at));
      return (b[sort] as number) - (a[sort] as number);
    });
    return data;
  }, [q.data, sort]);

  const th = (key: SortKey, label: string) => (
    <th
      className={`px-2 py-1 cursor-pointer select-none hover:text-ink ${sort === key ? "text-accent" : "text-muted"}`}
      onClick={() => setSort(key)}
    >
      {label} {sort === key ? "▾" : ""}
    </th>
  );

  return (
    <ChartCard
      title="Sessions"
      subtitle="Est. cost is API-equivalent (shadow) cost, not real spend. Click a row to drill in."
    >
      {sessionsProject && (
        <div className="mb-2 inline-flex items-center gap-2 text-xs rounded-md border border-line bg-elevated px-2 py-1">
          <span className="text-muted">Filtered to</span>
          <span className="text-ink font-mono">{sessionsProject}</span>
          <button className="text-muted hover:text-ink" onClick={() => setScreen("sessions")}>✕</button>
        </div>
      )}
      <table className="text-sm w-full">
        <thead>
          <tr className="text-left text-muted border-b border-line">
            <th className="px-2 py-1">Session</th>
            <th className="px-2 py-1">Project</th>
            {th("started_at", "Started")}
            {th("duration_secs", "Duration")}
            <th className="px-2 py-1">Messages</th>
            {th("tokens", "Tokens")}
            {th("cost_usd", "Est. cost")}
          </tr>
        </thead>
        <tbody>
          {rows.map((s: SessionSummary) => (
            <tr
              key={s.session_id}
              className="border-b border-line/50 hover:bg-elevated/50 cursor-pointer"
              onClick={() => openSession(s.session_id)}
            >
              <td className="px-2 py-1">
                <span className="text-ink">{s.title ?? shortId(s.session_id)}</span>
              </td>
              <td className="px-2 py-1 font-mono text-xs text-muted">{s.project}</td>
              <td className="px-2 py-1 tabular-nums text-muted">{startedLabel(s.started_at)}</td>
              <td className="px-2 py-1 tabular-nums">{dur(s.duration_secs)}</td>
              <td className="px-2 py-1 tabular-nums">{compact(s.message_count)}</td>
              <td className="px-2 py-1 tabular-nums">{compact(s.tokens)}</td>
              <td className="px-2 py-1 tabular-nums">{usd(s.cost_usd)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </ChartCard>
  );
}
