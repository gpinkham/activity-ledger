import { useQuery } from "@tanstack/react-query";
import { AreaChart, Area, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid } from "recharts";
import { getTotals, getByDay } from "../lib/ipc";
import { useFilters } from "../state/filters";
import { KpiRow } from "../components/KpiRow";
import { ChartCard } from "../components/ChartCard";
import { useChartTokens, VGradient } from "../lib/chart-theme";
import { usd, compact, pct } from "../lib/format";

export default function Overview() {
  const { filters } = useFilters();
  const tk = useChartTokens();
  const totals = useQuery({ queryKey: ["totals", filters], queryFn: () => getTotals(filters) });
  const byDay = useQuery({ queryKey: ["byDay", filters], queryFn: () => getByDay(filters) });

  const t = totals.data;
  return (
    <div className="space-y-4">
      <KpiRow
        items={[
          { label: "Sessions", value: t ? compact(t.sessions) : "-" },
          { label: "Messages", value: t ? compact(t.messages) : "-" },
          { label: "Output tokens", value: t ? compact(t.output_tokens) : "-" },
          { label: "Est. API-equiv. cost", value: t ? usd(t.est_cost_usd) : "-", hint: "what usage pricing would cost" },
          {
            label: "Cache hit ratio",
            value: t ? pct(t.cache_hit_ratio) : "-",
            tooltip:
              "Of the tokens that went through prompt caching, the share served from cache (cheap ~0.1× reads) vs freshly written to it (1.25 to 2× writes). Near 100% means the conversation prefix is reused very effectively across turns. Excludes fully-uncached input tokens.",
          },
          {
            label: "Est. $ saved by cache",
            value: t ? usd(t.saved_usd) : "-",
            tooltip:
              "Cached reads cost ~10% of the full input rate, so each saves ~90%. This is the estimated saving on those read tokens vs paying full price. It is a shadow figure (you're on a subscription, so nothing was actually billed).",
          },
        ]}
      />
      <ChartCard title="Est. API-equivalent cost over time" subtitle="Shadow cost, not real spend">
        <ResponsiveContainer width="100%" height={260}>
          <AreaChart data={byDay.data ?? []} margin={{ top: 8, right: 16, bottom: 0, left: 0 }}>
            <defs>
              <VGradient id="ov-cost" color={tk.ramp[0]} from={0.55} to={0.02} />
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
            <XAxis dataKey="day" tick={tk.tick} {...tk.axisLine} minTickGap={24} />
            <YAxis tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => usd(Number(v))} width={60} />
            <Tooltip formatter={(v) => usd(Number(v))} {...tk.tooltip} />
            <Area type="monotone" dataKey="cost_usd" name="Est. cost" stroke={tk.ramp[0]} strokeWidth={2} fill="url(#ov-cost)" />
          </AreaChart>
        </ResponsiveContainer>
      </ChartCard>
    </div>
  );
}
