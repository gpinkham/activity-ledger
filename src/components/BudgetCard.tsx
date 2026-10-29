// The menu bar budget gauge's settings: the monthly budget, the two thresholds and
// whether the ring shows in the menu bar. Saved with the other settings.
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChartCard } from "./ChartCard";
import { getSettings, setSettings } from "../lib/ipc";
import { cardDraft, checkBudget } from "../lib/gauge";
import { BUDGET_FIELDS, withOwnFields } from "../lib/settings";
import type { AppSettings } from "../lib/types";

const field = "border border-line rounded-md px-2 py-1 bg-surface text-ink w-full";

export function BudgetCard() {
  const qc = useQueryClient();
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings });
  // What was typed, until it is saved. Settings fetched again (after any save in the
  // app) do not overwrite it.
  const [edits, setEdits] = useState<AppSettings | null>(null);
  const saveMut = useMutation({
    mutationFn: (next: AppSettings) => setSettings(next),
    onSuccess: (_, next) => {
      qc.setQueryData(["settings"], next);
      setEdits(null);
      qc.invalidateQueries();
    },
  });
  const draft = cardDraft(edits, settings.data);
  if (!draft) return null;
  const set = <K extends keyof AppSettings>(k: K, v: AppSettings[K]) => setEdits({ ...draft, [k]: v });
  const problem = checkBudget(draft.monthly_budget_usd, draft.amber_pct, draft.red_pct);
  const daily = draft.monthly_budget_usd > 0 ? draft.monthly_budget_usd / 30.4 : 0;

  return (
    <ChartCard title="Menu bar budget"
      subtitle="A ring in the menu bar for today's spend against a monthly budget. An estimate from this Mac's Claude Code transcripts only.">
      <div className="grid grid-cols-3 gap-2 text-sm">
        <label className="text-muted">Monthly budget ($)
          <input type="number" min={0} step="1" className={field} value={draft.monthly_budget_usd}
            onChange={(e) => set("monthly_budget_usd", Number(e.target.value))} />
        </label>
        <label className="text-muted">Amber at (% of today)
          <input type="number" min={1} className={field} value={draft.amber_pct}
            onChange={(e) => set("amber_pct", Number(e.target.value))} />
        </label>
        <label className="text-muted">Red at (% of today)
          <input type="number" min={1} className={field} value={draft.red_pct}
            onChange={(e) => set("red_pct", Number(e.target.value))} />
        </label>
      </div>
      <label className="mt-3 flex items-center gap-2 text-sm text-ink">
        <input type="checkbox" checked={draft.show_in_menu_bar} onChange={(e) => set("show_in_menu_bar", e.target.checked)} />
        Show in menu bar (off by default)
      </label>
      <div className="mt-3 text-xs text-muted leading-relaxed">
        <p>Today&apos;s allowance is the budget spread evenly over the calendar month{daily > 0 ? `, about $${daily.toFixed(2)} a day` : ""}. Days start at local midnight.</p>
        <p>With the menu bar on, closing this window keeps Activity Ledger in the menu bar, out of the Dock. Click the ring for today&apos;s figures; its menu reopens this window or quits.</p>
        <p>Rates come from Claude Code&apos;s managed <code>modelPricing</code> setting when this Mac has one, otherwise from the price table. Nothing is sent anywhere.</p>
      </div>
      <button onClick={() => saveMut.mutate(withOwnFields(qc.getQueryData<AppSettings>(["settings"]), draft, BUDGET_FIELDS))} disabled={saveMut.isPending || problem !== null}
        className="mt-3 px-3 py-1 rounded-md text-white shadow-glow disabled:opacity-50"
        style={{ background: "linear-gradient(135deg, var(--c1), var(--c3))" }}>
        {saveMut.isPending ? "Saving…" : "Save"}
      </button>
      {problem && <span className="ml-2 text-xs text-red-500">{problem}</span>}
      {saveMut.error && <span className="ml-2 text-xs text-red-500">{String(saveMut.error)}</span>}
    </ChartCard>
  );
}
