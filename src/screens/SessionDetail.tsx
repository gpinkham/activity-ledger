import { useQuery } from "@tanstack/react-query";
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid } from "recharts";
import { getSessionDetail } from "../lib/ipc";
import { useNav } from "../state/nav";
import { ChartCard } from "../components/ChartCard";
import { KpiRow } from "../components/KpiRow";
import { Timeline } from "../components/Timeline";
import { useChartTokens, VGradient, HGradient } from "../lib/chart-theme";
import { usd, compact, pct } from "../lib/format";

function dur(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}
function stamp(s: string | null) { return s ? s.slice(0, 19).replace("T", " ") : "-"; }

export default function SessionDetail({ sessionId }: { sessionId: string }) {
  const { back } = useNav();
  const tk = useChartTokens();
  const q = useQuery({ queryKey: ["sessionDetail", sessionId], queryFn: () => getSessionDetail(sessionId) });
  const d = q.data;
  const s = d?.summary;

  const tokenBars = d
    ? [
        { name: "Input", value: d.tokens.input },
        { name: "Output", value: d.tokens.output },
        { name: "Cache write", value: d.tokens.cache_creation },
        { name: "Cache read", value: d.tokens.cache_read },
      ]
    : [];

  return (
    <div className="space-y-4">
      <div className="flex items-center gap-3">
        <button
          onClick={back}
          className="px-2.5 py-1 rounded-md border border-line text-muted hover:text-ink hover:bg-elevated text-sm"
        >
          ← Back
        </button>
        <div>
          <h1 className="display text-lg text-ink">{s?.title ?? sessionId.slice(0, 8)}</h1>
          <p className="text-xs text-muted">
            <span className="font-mono">{s?.project ?? ""}</span> · {stamp(s?.started_at ?? null)} → {stamp(s?.ended_at ?? null)} · {s ? dur(s.duration_secs) : ""}
          </p>
        </div>
      </div>

      <KpiRow
        items={[
          { label: "Messages", value: s ? compact(s.message_count) : "-" },
          { label: "Models", value: s ? String(s.models.length) : "-", hint: s?.models.join(", ") },
          { label: "Total tokens", value: s ? compact(s.tokens) : "-" },
          {
            label: "Cache hit ratio",
            value: d ? pct(d.cache_hit_ratio) : "-",
            tooltip:
              "For this session: of the tokens that went through prompt caching, the share served from cache (cheap ~0.1× reads) vs freshly written to it (1.25 to 2× writes). Near 100% means the growing prefix was reused effectively across turns. Excludes fully-uncached input tokens.",
          },
          { label: "Est. API-equiv. cost", value: s ? usd(s.cost_usd) : "-", hint: "shadow cost, not real spend" },
          { label: "Tool calls", value: s ? compact(s.tool_calls) : "-" },
        ]}
      />

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <ChartCard title="Token breakdown">
          <ResponsiveContainer width="100%" height={220}>
            <BarChart data={tokenBars}>
              <defs><VGradient id="sd-tokens" color={tk.ramp[2]} from={0.95} to={0.35} /></defs>
              <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
              <XAxis dataKey="name" tick={tk.tick} {...tk.axisLine} />
              <YAxis tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => compact(Number(v))} width={50} />
              <Tooltip formatter={(v) => compact(Number(v))} {...tk.tooltip} />
              <Bar dataKey="value" fill="url(#sd-tokens)" radius={[4, 4, 0, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </ChartCard>

        <ChartCard title="Est. cost per model (this session)" subtitle="Shadow cost, not real spend">
          <ResponsiveContainer width="100%" height={220}>
            <BarChart data={d?.by_model ?? []} layout="vertical" margin={{ left: 40 }}>
              <defs><HGradient id="sd-model" color={tk.ramp[0]} /></defs>
              <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
              <XAxis type="number" tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => usd(Number(v))} />
              <YAxis type="category" dataKey="model" tick={tk.tickSmall} {...tk.axisLine} width={140} />
              <Tooltip formatter={(v) => usd(Number(v))} {...tk.tooltip} />
              <Bar dataKey="cost_usd" fill="url(#sd-model)" radius={[0, 4, 4, 0]} />
            </BarChart>
          </ResponsiveContainer>
        </ChartCard>
      </div>

      <ChartCard title="Activity timeline" subtitle="What happened, in sequence. Metadata only, no message content">
        <Timeline events={d?.events ?? []} />
      </ChartCard>
    </div>
  );
}
