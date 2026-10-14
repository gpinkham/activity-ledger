import { useEffect, useMemo, useState } from "react";
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { save } from "@tauri-apps/plugin-dialog";
import { BarChart, Bar, XAxis, YAxis, Tooltip, ResponsiveContainer, CartesianGrid, Cell, Legend } from "recharts";
import { getActivity, getLabelProgress, getPriceTable, getSettings, runModelLabels, saveExport, setPriceTable, setSettings } from "../lib/ipc";
import { useFilters } from "../state/filters";
import { ChartCard } from "../components/ChartCard";
import { KpiRow } from "../components/KpiRow";
import { InfoTip } from "../components/InfoTip";
import { useChartTokens } from "../lib/chart-theme";
import { usd, compact, pct } from "../lib/format";
import type { ActivityRow, AppSettings, FastMode, LabelProgress, PriceTable } from "../lib/types";
import {
  coverageText, effectiveSource, groupByDay, hasModelLabels, readSavedSource, saveSource, shareOfLabelled,
  type LabelSource,
} from "../lib/labelSource";

const COST_LABEL = "Est. API cost (list price)";
const storage = () => (typeof localStorage === "undefined" ? undefined : localStorage);

/** Tool labels | LLM labels. The LLM side is off until the local model has labelled something. */
function LabelToggle({ value, llmReady, onChange }: { value: LabelSource; llmReady: boolean; onChange: (s: LabelSource) => void }) {
  const opts: { id: LabelSource; text: string; tip: string }[] = [
    { id: "rule", text: "Tool labels", tip: "Labels from the fixed rules: what started the segment, and its tool mix." },
    { id: "model", text: "LLM labels", tip: llmReady
      ? "Labels the local model gave each segment."
      : "No LLM labels yet: turn on the labelling pass below, save, and press Label now." },
  ];
  return (
    <div role="radiogroup" aria-label="Group costs by" className="inline-flex rounded-lg border border-line bg-surface p-0.5 text-sm">
      {opts.map((o) => {
        const on = value === o.id;
        const disabled = o.id === "model" && !llmReady;
        return (
          <button key={o.id} role="radio" aria-checked={on} disabled={disabled} title={o.tip}
            onClick={() => onChange(o.id)}
            className={`px-3 py-1 rounded-md ${on ? "bg-elevated text-ink" : "text-muted hover:text-ink"} disabled:opacity-40 disabled:cursor-not-allowed`}>
            {o.text}
          </button>
        );
      })}
    </div>
  );
}

/** One CSV per view (tool or LLM labels); labels, counts and costs only. */
function toCsv(rows: ActivityRow[]): string {
  const head = ["activity", "group", "segments", "calls", "input", "output", "cache_write", "cache_read",
    "cost_usd", "share", "ctx_p50", "ctx_p95", "out_p95", "local_candidate"];
  const esc = (v: string | number | boolean) => {
    const s = String(v);
    return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  };
  const lines = rows.map((r) => [r.activity, r.group, r.segments, r.calls, r.input, r.output, r.cache_write,
    r.cache_read, r.cost_usd.toFixed(4), r.share.toFixed(4), r.ctx_p50, r.ctx_p95, r.out_p95, r.local_candidate].map(esc).join(","));
  return [head.join(","), ...lines].join("\n") + "\n";
}

export default function Activity() {
  const { filters } = useFilters();
  const tk = useChartTokens();
  // The remembered choice, and whether the local model has labelled anything yet
  // (coverage is in every report, whichever view it is grouped by).
  const [want, setWant] = useState<LabelSource>(() => readSavedSource(storage()));
  const [llmReady, setLlmReady] = useState<boolean | null>(null);
  const source = effectiveSource(want, llmReady);
  // Keep showing the last report while another view loads, so the tab (and the
  // settings card's unsaved edits and progress) is not unmounted by a switch.
  const report = useQuery({
    queryKey: ["activity", filters, source], queryFn: () => getActivity(filters, source), placeholderData: keepPreviousData,
  });
  const [csvError, setCsvError] = useState<string | null>(null);
  const r = report.data;
  useEffect(() => { if (r) setLlmReady(hasModelLabels(r.coverage)); }, [r]);
  const choose = (s: LabelSource) => { setWant(s); saveSource(storage(), s); };
  const llm = r?.label_source === "model";

  const overheadColor = tk.ramp[5];
  const workColor = tk.ramp[1];
  const unlabelledColor = tk.muted;
  const colorOf = (g: string) => (g === "overhead" ? overheadColor : g === "unlabelled" ? unlabelledColor : workColor);

  const byDay = useMemo(() => groupByDay(r?.rows ?? [], r?.by_day ?? []), [r]);

  const exportCsv = async () => {
    setCsvError(null);
    try {
      const path = await save({ defaultPath: llm ? "cost-by-activity-llm-labels.csv" : "cost-by-activity.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
      if (path && r) await saveExport(path, toCsv(r.rows));
    } catch (e) {
      setCsvError(String(e));
    }
  };

  if (report.error) return <p className="text-sm text-red-500">{String(report.error)}</p>;
  if (!r) return <p className="text-sm text-muted">Loading…</p>;

  const t = r.totals;
  const diff = r.reconcile.claude_code_usd > 0 ? r.reconcile.ours_usd / r.reconcile.claude_code_usd - 1 : 0;
  const coverage = coverageText(r.coverage, usd);
  const labelsWord = llm ? "LLM labels" : "tool labels";
  // In the LLM view, shares are of the labelled cost (unlabelled is neither work nor overhead).
  const unlabelled = llm ? t.cost_usd - r.coverage.labelled_usd : 0;
  const ofCost = (part: number) => shareOfLabelled(part, t.cost_usd, unlabelled);
  const ofLabelled = (v: string) => (llm ? `${v} of ${usd(r.coverage.labelled_usd)} labelled` : v);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <LabelToggle value={source} llmReady={!!llmReady} onChange={choose} />
        <span className="text-xs text-muted">
          {coverage ?? "No LLM labels yet. The labelling pass is under Local model."}
        </span>
      </div>
      <KpiRow
        items={[
          { label: "Est. API cost", value: usd(t.cost_usd), hint: "list price, not a bill" },
          { label: "Overhead", value: pct(ofCost(t.overhead_usd)), hint: ofLabelled(usd(t.overhead_usd)),
            tooltip: llm
              ? "Segments the local model labelled overhead, as a share of the cost it labelled. Unlabelled segments are left out of both."
              : "Context compactions: cost that goes to keeping the session going, not to the task." },
          { label: "Local-model candidates", value: pct(ofCost(t.local_candidate_usd)), hint: ofLabelled(usd(t.local_candidate_usd)),
            tooltip: "Cost in activity types whose p95 context and output fit the local model limits set below. A hint, not a verdict." },
          { label: "API calls", value: compact(t.calls) },
          { label: "Work segments", value: compact(t.segments),
            tooltip: "A segment is the work done in answer to one inbound prompt." },
          { label: "Check vs Claude Code", value: r.reconcile.sessions ? `${diff >= 0 ? "+" : ""}${pct(diff)}` : "-",
            hint: r.reconcile.sessions ? `${r.reconcile.sessions} sessions` : "no cost-state records",
            tooltip: "Our cost for the calls each Claude Code cost snapshot covers (since its process started), against the snapshot's own total. Ignores the date filter. A large gap means a parsing or pricing problem." },
        ]}
      />
      {r.unknown_models.length > 0 && (
        <div className="text-xs text-muted rounded-lg border border-line bg-surface px-3 py-2">
          <p>Priced at the default row (unknown to the price table): {r.unknown_models.join(", ")}</p>
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <ChartCard title="Cost by activity" subtitle={`${COST_LABEL}, by ${labelsWord}`}>
          <ResponsiveContainer width="100%" height={Math.max(220, r.rows.length * 26)}>
            <BarChart data={r.rows} layout="vertical" margin={{ left: 30 }}>
              <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
              <XAxis type="number" tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => usd(Number(v))} />
              <YAxis type="category" dataKey="activity" tick={tk.tickSmall} {...tk.axisLine} width={120} />
              <Tooltip formatter={(v) => usd(Number(v))} {...tk.tooltip} />
              <Bar dataKey="cost_usd" name="cost" radius={[0, 4, 4, 0]}>
                {r.rows.map((x) => <Cell key={x.activity} fill={colorOf(x.group)} />)}
              </Bar>
            </BarChart>
          </ResponsiveContainer>
          <div className="mt-2 flex gap-4 text-[11px] text-muted">
            <span><span className="inline-block w-2 h-2 rounded-sm mr-1" style={{ background: workColor }} />work</span>
            <span><span className="inline-block w-2 h-2 rounded-sm mr-1" style={{ background: overheadColor }} />overhead</span>
            {llm && <span><span className="inline-block w-2 h-2 rounded-sm mr-1" style={{ background: unlabelledColor }} />not labelled by the model</span>}
          </div>
        </ChartCard>

        <ChartCard title="Over time" subtitle={`${COST_LABEL}, per day, by ${labelsWord}`}>
          <ResponsiveContainer width="100%" height={260}>
            <BarChart data={byDay}>
              <CartesianGrid strokeDasharray="3 3" stroke={tk.grid} />
              <XAxis dataKey="day" tick={tk.tickSmall} {...tk.axisLine} />
              <YAxis tick={tk.tick} {...tk.axisLine} tickFormatter={(v) => usd(Number(v))} width={60} />
              <Tooltip formatter={(v) => usd(Number(v))} {...tk.tooltip} />
              <Legend wrapperStyle={{ fontSize: 11 }} />
              <Bar dataKey="work" stackId="d" fill={workColor} />
              <Bar dataKey="overhead" stackId="d" fill={overheadColor} radius={llm ? undefined : [4, 4, 0, 0]} />
              {llm && <Bar dataKey="unlabelled" name="not labelled" stackId="d" fill={unlabelledColor} radius={[4, 4, 0, 0]} />}
            </BarChart>
          </ResponsiveContainer>
        </ChartCard>
      </div>

      <ChartCard title="By activity" subtitle="Context = input + cache write + cache read per call">
        <div className="flex items-center gap-2 mb-2">
          <button onClick={exportCsv} className="px-3 py-1 rounded-md border border-line text-sm text-ink hover:bg-elevated">Export CSV</button>
          {csvError && <span className="text-xs text-red-500">{csvError}</span>}
        </div>
        <table className="text-sm w-full">
          <thead>
            <tr className="text-left text-muted">
              <th className="py-1">Activity</th><th>Group</th><th className="text-right">Segments</th><th className="text-right">Calls</th>
              <th className="text-right">Cost</th><th className="text-right">Share</th>
              <th className="text-right">Context p50</th><th className="text-right">p95</th><th className="text-right">Output p95</th>
              <th className="pl-4">Local model?</th>
              <th className="pl-4">{llm ? "Tool labels" : "Local model label"}</th>
            </tr>
          </thead>
          <tbody>
            {r.rows.map((x) => (
              <tr key={x.activity} className="border-t border-line/60 tabular-nums">
                <td className="py-1 text-ink">{x.activity}</td>
                <td className="text-muted">{x.group}</td>
                <td className="text-right">{compact(x.segments)}</td>
                <td className="text-right">{compact(x.calls)}</td>
                <td className="text-right text-ink">{usd(x.cost_usd)}</td>
                <td className="text-right">{pct(x.share)}</td>
                <td className="text-right">{compact(x.ctx_p50)}</td>
                <td className="text-right">{compact(x.ctx_p95)}</td>
                <td className="text-right">{compact(x.out_p95)}</td>
                <td className="pl-4">
                  <span className={x.local_candidate ? "text-ink" : "text-muted"}>{x.local_candidate ? "candidate" : "no"}</span>
                  <InfoTip text={x.local_reasons.join(" · ")} label={`Why for ${x.activity}`} />
                </td>
                <td className="pl-4 text-muted">
                  {((llm ? x.rule_labels : x.model_labels) ?? []).slice(0, 3).map((m) => (
                    <span key={m.label} className={m.label === x.activity ? "" : "text-ink"}>{m.label} {m.segments} </span>
                  ))}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </ChartCard>

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
        <SettingsCard />
        <PricingCard />
      </div>
    </div>
  );
}

const field = "border border-line rounded-md px-2 py-1 bg-surface text-ink w-full";

/** A token limit as the backend's whole number: an emptied or invalid field keeps
 *  the saved value, a decimal is rounded. */
const tokenLimit = (v: number, saved: number) => (Number.isFinite(v) && v >= 1 ? Math.round(v) : saved);

function SettingsCard() {
  const qc = useQueryClient();
  const { filters } = useFilters();
  const [progress, setProgress] = useState<LabelProgress | null>(null);
  const [labelError, setLabelError] = useState<string | null>(null);
  useEffect(() => {
    if (!progress?.running) return;
    const iv = setInterval(async () => {
      const p = await getLabelProgress();
      setProgress(p);
      if (!p.running) qc.invalidateQueries({ queryKey: ["activity"] });
    }, 1000);
    return () => clearInterval(iv);
  }, [progress?.running, qc]);
  const labelNow = async () => {
    setLabelError(null);
    try {
      await runModelLabels(filters);
      setProgress(await getLabelProgress());
    } catch (e) {
      setLabelError(String(e));
    }
  };
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings });
  const [draft, setDraft] = useState<AppSettings | null>(null);
  useEffect(() => { if (settings.data) setDraft(settings.data); }, [settings.data]);
  const saveMut = useMutation({ mutationFn: setSettings, onSuccess: () => qc.invalidateQueries() });
  if (!draft) return null;
  const set = <K extends keyof AppSettings>(k: K, v: AppSettings[K]) => setDraft({ ...draft, [k]: v });

  return (
    <ChartCard title="Local model" subtitle="Saved on this machine. Only Label now sends anything, and only to this Ollama.">
      <div className="grid grid-cols-2 gap-2 text-sm">
        <label className="text-muted">Ollama URL
          <input className={field} value={draft.ollama_url} onChange={(e) => set("ollama_url", e.target.value)} />
        </label>
        <label className="text-muted">Ollama model
          <input className={field} value={draft.ollama_model} placeholder="not set" onChange={(e) => set("ollama_model", e.target.value)} />
        </label>
        <label className="text-muted">Local context limit (tokens)
          <input type="number" className={field} value={draft.local_context_limit}
            onChange={(e) => set("local_context_limit", Number(e.target.value))} />
        </label>
        <label className="text-muted">Local output limit (tokens)
          <input type="number" className={field} value={draft.local_output_limit}
            onChange={(e) => set("local_output_limit", Number(e.target.value))} />
        </label>
      </div>
      <div className="mt-3 text-xs text-muted leading-relaxed">
        <p><b className="text-ink">Context limit</b>: the most text the model can read at once: the conversation, the system prompt, tool definitions and any files, together.</p>
        <p><b className="text-ink">Output limit</b>: the longest reply it will write.</p>
        <p>Check a model with <code>ollama show &lt;model&gt;</code>. Ollama runs it with its own <code>num_ctx</code>, often well below the model&apos;s maximum: the limit you actually run with is the one that counts. On a laptop, memory usually caps the context at about 32k to 64k tokens.</p>
        <p>The &quot;local model?&quot; flag compares each label&apos;s p95 call size (context and output per call) with these two limits.</p>
      </div>
      <div className="mt-3 flex flex-col gap-1 text-sm">
        <label className="flex items-center gap-2 text-ink">
          <input type="checkbox" checked={draft.model_labels_enabled} onChange={(e) => set("model_labels_enabled", e.target.checked)} />
          Label segments with the local model (Ollama only; off by default)
        </label>
        <label className="flex items-center gap-2 text-ink">
          <input type="checkbox" checked={draft.allow_remote_ollama} onChange={(e) => set("allow_remote_ollama", e.target.checked)} />
          Allow a remote Ollama
        </label>
        {draft.allow_remote_ollama && (
          <p className="text-xs text-red-500">A remote Ollama receives an excerpt of each segment&apos;s opening prompt: that text leaves this machine.</p>
        )}
      </div>
      <button
        onClick={() => saveMut.mutate({
          ...draft,
          local_context_limit: tokenLimit(draft.local_context_limit, settings.data!.local_context_limit),
          local_output_limit: tokenLimit(draft.local_output_limit, settings.data!.local_output_limit),
        })}
        disabled={saveMut.isPending}
        className="mt-3 px-3 py-1 rounded-md text-white shadow-glow disabled:opacity-50"
        style={{ background: "linear-gradient(135deg, var(--c1), var(--c3))" }}>
        {saveMut.isPending ? "Saving…" : "Save"}
      </button>
      {saveMut.error && <span className="ml-2 text-xs text-red-500">{String(saveMut.error)}</span>}
      {settings.data?.model_labels_enabled && (
        <div className="mt-3 flex items-center gap-2 text-sm">
          <button onClick={labelNow} disabled={!!progress?.running}
            className="px-3 py-1 rounded-md border border-line text-ink hover:bg-elevated disabled:opacity-50">
            {progress?.loading ? "Loading the model…" : progress?.running ? "Labelling…" : "Label now"}
          </button>
          {progress && <span className="text-xs text-muted">labelled {progress.labelled} of {progress.total}{progress.failed ? `, ${progress.failed} failed` : ""}</span>}
          {progress?.stopped && <span className="text-xs text-red-500">{progress.stopped}</span>}
          {labelError && <span className="text-xs text-red-500">{labelError}</span>}
        </div>
      )}
    </ChartCard>
  );
}

function PricingCard() {
  const qc = useQueryClient();
  const table = useQuery({ queryKey: ["priceTable"], queryFn: getPriceTable });
  const saveMut = useMutation({ mutationFn: (t: PriceTable) => setPriceTable(t), onSuccess: () => qc.invalidateQueries() });
  const t = table.data;
  if (!t) return null;
  return (
    <ChartCard title="Pricing assumptions" subtitle={`Direct Claude API list prices${t.prices_as_of ? ` as of ${t.prices_as_of}` : ""}; edit rows on Cost & efficiency`}>
      <div className="flex flex-col gap-3 text-sm">
        <label className="flex items-center gap-2 text-ink">
          <input type="checkbox" checked={!!t.us_only} onChange={(e) => saveMut.mutate({ ...t, us_only: e.target.checked })} />
          US-only inference (1.1× on every token)
        </label>
        <label className="text-muted">Fast mode
          <select className={field} value={t.fast_mode ?? "auto"}
            onChange={(e) => saveMut.mutate({ ...t, fast_mode: e.target.value as FastMode })}>
            <option value="auto">As each call recorded it (default)</option>
            <option value="never">Never</option>
            <option value="always">Always, on models that offer it</option>
          </select>
        </label>
        {saveMut.error && <span className="text-xs text-red-500">{String(saveMut.error)}</span>}
      </div>
    </ChartCard>
  );
}
