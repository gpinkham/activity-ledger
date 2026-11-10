// The menu bar gauge's settings that are the machine's, not an account's: whether
// the ring shows in the menu bar, and which account it follows. Each account's
// budget is set in the Accounts card. Saved with the other settings.
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChartCard } from "./ChartCard";
import { getSettings, setSettings, setTrayAccount } from "../lib/ipc";
import { cardDraft, words } from "../lib/gauge";
import { BUDGET_FIELDS, withOwnFields } from "../lib/settings";
import { accountById, visibleAccounts } from "../lib/accounts";
import { useAccounts } from "./Accounts";
import type { AppSettings } from "../lib/types";

const field = "border border-line rounded-md px-2 py-1 bg-surface text-ink w-full";

export function BudgetCard() {
  const qc = useQueryClient();
  const settings = useQuery({ queryKey: ["settings"], queryFn: getSettings });
  const accounts = useAccounts();
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
  // Saved at once: only this one field is written.
  const trayMut = useMutation({
    mutationFn: (account: string | null) => setTrayAccount(account),
    onSuccess: (list) => {
      qc.setQueryData(["accounts"], list);
      qc.invalidateQueries({ queryKey: ["settings"] });
    },
  });
  const draft = cardDraft(edits, settings.data);
  if (!draft) return null;
  const set = <K extends keyof AppSettings>(k: K, v: AppSettings[K]) => setEdits({ ...draft, [k]: v });
  const follows = settings.data?.tray_account ?? null;
  const followed = accountById(accounts.data, follows);
  const budget = followed?.settings.monthly_budget_usd ?? 0;

  return (
    <ChartCard title={`${words.Bar} budget`}
      subtitle={`A ring in the ${words.bar} for today's spend against one account's monthly budget. An estimate from this ${words.machine}'s Claude Code transcripts only.`}>
      <label className="flex items-center gap-2 text-sm text-ink">
        <input type="checkbox" checked={draft.show_in_menu_bar} onChange={(e) => set("show_in_menu_bar", e.target.checked)} />
        Show in {words.bar} (off by default)
      </label>
      <label className="mt-3 block text-sm text-muted">The ring follows
        <select className={field} value={follows ?? ""} disabled={trayMut.isPending}
          onChange={(e) => trayMut.mutate(e.target.value || null)}>
          <option value="">All accounts (spend only, no ring)</option>
          {visibleAccounts(accounts.data).map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}{a.settings.monthly_budget_usd > 0 ? `: $${a.settings.monthly_budget_usd.toFixed(0)} a month` : ": no budget set"}
            </option>
          ))}
        </select>
      </label>
      <div className="mt-3 text-xs text-muted leading-relaxed">
        <p>Each account has its own budget and thresholds, set below under Accounts. {followed
          ? budget > 0
            ? `${followed.label}'s allowance is its budget spread evenly over the calendar month, about $${(budget / 30.4).toFixed(2)} a day.`
            : `${followed.label} has no budget yet, so the ring stays grey.`
          : "With all accounts the ring has no budget to measure against: the panel shows their spend together."} Days start at local midnight. The panel can switch accounts too.</p>
        <p>With the {words.bar} on, closing this window keeps Activity Ledger in the {words.bar}, out of the {words.dock}. Click the ring for today&apos;s figures, with buttons to reopen this window or quit.</p>
        <p>Rates come from Claude Code&apos;s managed <code>modelPricing</code> setting when this {words.machine} has one, otherwise from the price table. Nothing is sent anywhere.</p>
      </div>
      <button onClick={() => saveMut.mutate(withOwnFields(qc.getQueryData<AppSettings>(["settings"]), draft, BUDGET_FIELDS))} disabled={saveMut.isPending}
        className="mt-3 px-3 py-1 rounded-md text-white shadow-glow disabled:opacity-50"
        style={{ background: "linear-gradient(135deg, var(--c1), var(--c3))" }}>
        {saveMut.isPending ? "Saving…" : "Save"}
      </button>
      {saveMut.error && <span className="ml-2 text-xs text-red-500">{String(saveMut.error)}</span>}
      {trayMut.error && <span className="ml-2 text-xs text-red-500">{String(trayMut.error)}</span>}
    </ChartCard>
  );
}
