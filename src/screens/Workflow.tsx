import { lazy, Suspense, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid, AreaChart, Area, Legend } from "recharts";
import { getByTool, getWorkflow } from "../lib/ipc";
import { useFilters } from "../state/filters";
import { ChartCard } from "../components/ChartCard";
import { Heatmap } from "../components/Heatmap";
import { KpiRow } from "../components/KpiRow";
import { useChartTokens, VGradient, HGradient } from "../lib/chart-theme";
import { compact, pct } from "../lib/format";

const Terrain3D = lazy(() => import("../components/Terrain3D"));

export default function WorkflowScreen() {
  const { filters } = useFilters();
  const tk = useChartTokens();
  const [heatMode, setHeatMode] = useState<"3d" | "flat">("3d");
  const tools = useQuery({ queryKey: ["byTool", filters], queryFn: () => getByTool(filters) });
  const wf = useQuery({ queryKey: ["workflow", filters], queryFn: () => getWorkflow(filters) });

  const w = wf.data;
  const modifiedRate = w && w.edit_write_total > 0 ? w.user_modified / w.edit_write_total : 0;
  const bashRatio = w && w.edit_write > 0 ? w.bash / w.edit_write : 0;

  const modelColors = tk.ramp;
  const models = Array.from(new Set((w?.model_mix ?? []).map((m) => m.model)));
  const mixByDay: Record<string, Record<string, number | string>> = {};
  for (const r of w?.model_mix ?? []) {
    if (!mixByDay[r.day]) mixByDay[r.day] = { day: r.day };
    mixByDay[r.day][r.model] = r.messages;
  }
  const mixData = Object.values(mixByDay).sort((a, b) => String(a.day).localeCompare(String(b.day)));

  return (
    <div className="space-y-4">
      <KpiRow
        items={[
          { label: "Bash calls", value: w ? compact(w.bash) : "-" },
          { label: "Edit/Write calls", value: w ? compact(w.edit_write) : "-" },
          { label: "Bash : Edit ratio", value: w ? bashRatio.toFixed(2) : "-", hint: "explore vs build" },
          { label: "userModified rate", value: w ? pct(modifiedRate) : "-", hint: "how often you corrected edits" },
          { label: "Web searches", value: w ? compact(w.web_search) : "-" },
          { label: "Web fetches", value: w ? compact(w.web_fetch) : "-" },
        ]}
      />
      <ChartCard title="Tool usage">
        <ResponsiveContainer width="100%" height={320}>
          <BarChart data={(tools.data ?? []).slice(0, 20)} layout="vertical" margin={{ left: 40 }}>
            <defs>
              <HGradient id="wf-tool" color={tk.ramp[1]} />
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
            <XAxis type="number" tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => compact(Number(v))} />
            <YAxis type="category" dataKey="tool" tick={tk.tickSmall} {...tk.axisLine} width={160} />
            <Tooltip formatter={(v) => compact(Number(v))} {...tk.tooltip} />
            <Bar dataKey="count" name="Calls" fill="url(#wf-tool)" radius={[0, 4, 4, 0]} />
          </BarChart>
        </ResponsiveContainer>
      </ChartCard>
      <ChartCard title="Activity heatmap (hour × weekday)" subtitle="Messages by hour (UTC) and weekday. Drag the terrain to orbit, scroll to zoom">
        <div className="mb-2 inline-flex rounded-md border border-line overflow-hidden text-xs">
          {(["3d", "flat"] as const).map((m) => (
            <button
              key={m}
              onClick={() => setHeatMode(m)}
              className={`px-3 py-1 transition-colors ${
                heatMode === m ? "bg-elevated text-ink" : "text-muted hover:text-ink"
              }`}
            >
              {m === "3d" ? "3D terrain" : "Flat"}
            </button>
          ))}
        </div>
        {heatMode === "3d" ? (
          <Suspense fallback={<div className="h-[340px] grid place-items-center text-sm text-muted">Loading terrain…</div>}>
            <Terrain3D cells={w?.heatmap ?? []} />
          </Suspense>
        ) : (
          <Heatmap cells={w?.heatmap ?? []} />
        )}
      </ChartCard>
      <ChartCard title="Model mix over time" subtitle="Messages per model per day">
        <ResponsiveContainer width="100%" height={280}>
          <AreaChart data={mixData} margin={{ top: 8, right: 16, bottom: 0, left: 0 }}>
            <defs>
              {models.map((m, i) => (
                <VGradient key={m} id={`wf-mix-${i}`} color={modelColors[i % modelColors.length]} from={0.7} to={0.08} />
              ))}
            </defs>
            <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
            <XAxis dataKey="day" tick={tk.tick} {...tk.axisLine} minTickGap={24} />
            <YAxis tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => compact(Number(v))} width={50} />
            <Tooltip formatter={(v) => compact(Number(v))} {...tk.tooltip} />
            <Legend wrapperStyle={{ fontSize: 11, color: tk.muted }} />
            {models.map((m, i) => (
              <Area key={m} type="monotone" dataKey={m} stackId="1"
                stroke={modelColors[i % modelColors.length]} strokeWidth={1.5}
                fill={`url(#wf-mix-${i})`} />
            ))}
          </AreaChart>
        </ResponsiveContainer>
      </ChartCard>
    </div>
  );
}
