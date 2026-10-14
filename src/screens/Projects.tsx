import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getByProject } from "../lib/ipc";
import { useFilters } from "../state/filters";
import { useNav } from "../state/nav";
import { ChartCard } from "../components/ChartCard";
import { usd, compact } from "../lib/format";
import type { ByProject } from "../lib/types";

type SortKey = "cost_usd" | "tokens" | "sessions" | "tool_calls";

export default function Projects() {
  const { filters } = useFilters();
  const { openSessionsForProject } = useNav();
  const [sort, setSort] = useState<SortKey>("cost_usd");
  const q = useQuery({ queryKey: ["byProject", filters], queryFn: () => getByProject(filters) });

  const rows = useMemo(() => {
    const data = [...(q.data ?? [])];
    data.sort((a, b) => (b[sort] as number) - (a[sort] as number));
    return data;
  }, [q.data, sort]);

  const header = (key: SortKey, label: string) => (
    <th
      className={`px-2 py-1 cursor-pointer select-none hover:text-ink ${sort === key ? "text-accent" : "text-muted"}`}
      onClick={() => setSort(key)}
    >
      {label} {sort === key ? "▾" : ""}
    </th>
  );

  // research-vs-code signature: share of web tool calls among all tool calls.
  const signature = (p: ByProject) => (p.tool_calls > 0 ? p.web / p.tool_calls : 0);

  return (
    <ChartCard title="Projects" subtitle="Est. cost is API-equivalent (shadow) cost, not real spend. Click a column header to sort, or a row to see its sessions.">
      <table className="text-sm w-full">
        <thead>
          <tr className="text-left text-muted border-b border-line">
            <th className="px-2 py-1">Project</th>
            {header("sessions", "Sessions")}
            {header("tokens", "Tokens")}
            {header("cost_usd", "Est. cost")}
            {header("tool_calls", "Tool calls")}
            <th className="px-2 py-1">Research-ish</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((p) => (
            <tr
              key={p.project}
              className="border-b border-line/50 hover:bg-elevated/50 cursor-pointer"
              onClick={() => openSessionsForProject(p.project)}
            >
              <td className="px-2 py-1 font-mono text-xs">{p.project}</td>
              <td className="px-2 py-1 tabular-nums">{compact(p.sessions)}</td>
              <td className="px-2 py-1 tabular-nums">{compact(p.tokens)}</td>
              <td className="px-2 py-1 tabular-nums">{usd(p.cost_usd)}</td>
              <td className="px-2 py-1 tabular-nums">{compact(p.tool_calls)}</td>
              <td className="px-2 py-1 tabular-nums">{(signature(p) * 100).toFixed(0)}%</td>
            </tr>
          ))}
        </tbody>
      </table>
    </ChartCard>
  );
}
