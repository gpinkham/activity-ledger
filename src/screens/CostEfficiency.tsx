import { useQuery } from "@tanstack/react-query";
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid } from "recharts";
import { getTotals, getByModel, getByProject } from "../lib/ipc";
import { useFilters } from "../state/filters";
import { ChartCard } from "../components/ChartCard";
import { PriceTableEditor } from "../components/PriceTableEditor";
import { InfoTip } from "../components/InfoTip";
import { useChartTokens, VGradient, HGradient } from "../lib/chart-theme";
import { usd, compact } from "../lib/format";

export default function CostEfficiency() {
  const { filters } = useFilters();
  const tk = useChartTokens();
  const totals = useQuery({ queryKey: ["totals", filters], queryFn: () => getTotals(filters) });
  const byModel = useQuery({ queryKey: ["byModel", filters], queryFn: () => getByModel(filters) });
  const byProject = useQuery({ queryKey: ["byProject", filters], queryFn: () => getByProject(filters) });

  const t = totals.data;
  const tokenBreakdown = t
    ? [
        { name: "Input", value: t.input_tokens },
        { name: "Output", value: t.output_tokens },
        { name: "Cache write", value: t.cache_creation },
        { name: "Cache read", value: t.cache_read },
      ]
    : [];

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
      <ChartCard title="Token breakdown">
        <ResponsiveContainer width="100%" height={240}>
          <BarChart data={tokenBreakdown}>
            <defs>
              <VGradient id="ce-tokens" color={tk.ramp[2]} from={0.95} to={0.35} />
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
            <XAxis dataKey="name" tick={tk.tick} {...tk.axisLine} />
            <YAxis tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => compact(Number(v))} width={50} />
            <Tooltip formatter={(v) => compact(Number(v))} {...tk.tooltip} />
            <Bar dataKey="value" name="Tokens" fill="url(#ce-tokens)" radius={[4, 4, 0, 0]} />
          </BarChart>
        </ResponsiveContainer>
        <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-[11px] text-muted">
          <span className="inline-flex items-center">
            Cache write
            <InfoTip
              text="Tokens written into Anthropic's server-side prompt cache this turn (a cache write), billed at a 1.25× (5-minute) or 2× (1-hour) premium over the input rate."
              label="About cache write"
            />
          </span>
          <span className="inline-flex items-center">
            Cache read
            <InfoTip
              text="Tokens served from the server-side prompt cache (a cache hit), billed at only ~0.1× the input rate (a ~90% discount). Cache reads are where the savings come from."
              label="About cache read"
            />
          </span>
        </div>
      </ChartCard>

      <ChartCard title="Est. API-equivalent cost per model" subtitle="Shadow cost, not real spend">
        <ResponsiveContainer width="100%" height={240}>
          <BarChart data={byModel.data ?? []} layout="vertical" margin={{ left: 40 }}>
            <defs>
              <HGradient id="ce-model" color={tk.ramp[0]} />
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
            <XAxis type="number" tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => usd(Number(v))} />
            <YAxis type="category" dataKey="model" tick={tk.tickSmall} {...tk.axisLine} width={140} />
            <Tooltip formatter={(v) => usd(Number(v))} {...tk.tooltip} />
            <Bar dataKey="cost_usd" name="Est. cost" fill="url(#ce-model)" radius={[0, 4, 4, 0]} />
          </BarChart>
        </ResponsiveContainer>
      </ChartCard>

      <ChartCard title="Est. cost per project" subtitle="Shadow cost, not real spend">
        <ResponsiveContainer width="100%" height={280}>
          <BarChart data={(byProject.data ?? []).slice(0, 12)} layout="vertical" margin={{ left: 40 }}>
            <defs>
              <HGradient id="ce-project" color={tk.ramp[3]} />
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
            <XAxis type="number" tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => usd(Number(v))} />
            <YAxis type="category" dataKey="project" tick={{ ...tk.tickSmall, fontSize: 9 }} {...tk.axisLine} width={160} />
            <Tooltip formatter={(v) => usd(Number(v))} {...tk.tooltip} />
            <Bar dataKey="cost_usd" name="Est. cost" fill="url(#ce-project)" radius={[0, 4, 4, 0]} />
          </BarChart>
        </ResponsiveContainer>
      </ChartCard>

      <ChartCard title="Model pricing (per 1M tokens, editable)" subtitle="Estimated API-equivalent cost, not real spend. Cache read ×0.1, 5m write ×1.25, 1h write ×2.0 of input rate">
        <PriceTableEditor />
      </ChartCard>
    </div>
  );
}
