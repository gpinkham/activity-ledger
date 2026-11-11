// The Claude accounts found on this machine: each one's name, colour and budget,
// the config folders counted in it, and folders to add by hand. Each account saves
// on its own (set_account_settings), so saving one cannot undo another.
import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChartCard } from "./ChartCard";
import { AccountDot, useAccounts } from "./Accounts";
import { addConfigDir, removeConfigDir, setAccountSettings, setDirAccount } from "../lib/ipc";
import { accountDetail, accountOrigins, checkColour } from "../lib/accounts";
import { checkBudget, words } from "../lib/gauge";
import type { Account, AccountSettings } from "../lib/types";

const field = "border border-line rounded-md px-2 py-1 bg-surface text-ink w-full";
const BUTTON = "px-3 py-1 rounded-md border border-line text-ink hover:bg-elevated disabled:opacity-50";

export function AccountsCard() {
  const qc = useQueryClient();
  const accounts = useAccounts();
  const [newDir, setNewDir] = useState("");
  // Every view depends on which folders count where, so a change there reloads all.
  const changed = (list: Account[]) => {
    qc.setQueryData(["accounts"], list);
    void qc.invalidateQueries();
  };
  const dirMut = useMutation({
    mutationFn: ({ dir, account }: { dir: string; account: string | null }) => setDirAccount(dir, account),
    onSuccess: changed,
  });
  const addMut = useMutation({
    mutationFn: (path: string) => addConfigDir(path),
    onSuccess: (list) => {
      setNewDir("");
      changed(list);
    },
  });
  const removeMut = useMutation({ mutationFn: (path: string) => removeConfigDir(path), onSuccess: changed });
  const list = accounts.data ?? [];
  const error = dirMut.error ?? addMut.error ?? removeMut.error;

  return (
    <ChartCard title="Accounts"
      subtitle={`The Claude subscriptions on this ${words.machine}. Each Claude Code config folder (~/.claude, a CLAUDE_CONFIG_DIR such as ~/.claude-work, in WSL too) belongs to the account it is signed in to; folders signed in to the same account are counted together.`}>
      {accounts.isLoading && <p className="text-sm text-muted">Looking for config folders…</p>}
      {!accounts.isLoading && list.length === 0 && (
        <p className="text-sm text-muted">No Claude Code config folder with transcripts was found. Add one below.</p>
      )}
      <div className="grid grid-cols-1 xl:grid-cols-2 gap-3">
        {list.map((a) => (
          <AccountPanel key={a.id} account={a} all={list}
            onMove={(dir, account) => dirMut.mutate({ dir, account })}
            onRemove={(dir) => removeMut.mutate(dir)}
            busy={dirMut.isPending || removeMut.isPending} />
        ))}
      </div>
      <div className="mt-4 text-sm">
        <label className="text-muted">Add a config folder that was not found (the folder CLAUDE_CONFIG_DIR names, or its projects folder)
          <span className="mt-1 flex gap-2">
            <input className={field} value={newDir} placeholder={String.raw`\\wsl.localhost\Ubuntu\home\me\.claude-client`}
              onChange={(e) => setNewDir(e.target.value)} />
            <button className={BUTTON} disabled={!newDir.trim() || addMut.isPending} onClick={() => addMut.mutate(newDir.trim())}>
              {addMut.isPending ? "Reading…" : "Add"}
            </button>
          </span>
        </label>
        <p className="mt-2 text-xs text-muted leading-relaxed">
          An account is known by the login in its folder&apos;s <code>.claude.json</code>; the credentials file is never read.
          A folder signed out and back in as someone else counts all its history in the login it has now. Refresh looks for new folders.
        </p>
        {error && <p className="mt-1 text-xs text-red-500">{String(error)}</p>}
      </div>
    </ChartCard>
  );
}

function AccountPanel({ account: a, all, onMove, onRemove, busy }: {
  account: Account; all: Account[]; busy: boolean;
  onMove: (dir: string, account: string | null) => void;
  onRemove: (dir: string) => void;
}) {
  const qc = useQueryClient();
  // What was typed, until saved: a refetch after another account's save keeps it.
  const [edits, setEdits] = useState<AccountSettings | null>(null);
  const draft: AccountSettings = edits ?? { ...a.settings, colour: a.settings.colour ?? a.colour };
  const set = <K extends keyof AccountSettings>(k: K, v: AccountSettings[K]) => setEdits({ ...draft, [k]: v });
  const saveMut = useMutation({
    mutationFn: () => setAccountSettings(a.id, { ...draft, label: draft.label?.trim() || null }),
    onSuccess: (list) => {
      qc.setQueryData(["accounts"], list);
      setEdits(null);
      // A hidden account leaves every "All accounts" figure.
      void qc.invalidateQueries();
    },
  });
  const problem = checkBudget(draft.monthly_budget_usd, draft.amber_pct, draft.red_pct)
    ?? (draft.colour ? checkColour(draft.colour) : null);

  return (
    <section className={`rounded-md border border-line p-3 ${a.settings.hidden ? "opacity-60" : ""}`} data-account={a.id}>
      <header className="flex items-start gap-2">
        <span className="mt-1.5"><AccountDot colour={draft.colour ?? a.colour} size={10} /></span>
        <div className="min-w-0">
          <div className="text-ink font-medium truncate">{a.label}{a.settings.hidden ? " (hidden)" : ""}</div>
          <div className="text-xs text-muted truncate">{[accountDetail(a), accountOrigins(a)].filter(Boolean).join(" · ")}</div>
        </div>
      </header>

      <div className="mt-3 grid grid-cols-[1fr_auto] gap-2 text-sm">
        <label className="text-muted">Name
          <input className={field} value={draft.label ?? ""} placeholder={a.default_label}
            onChange={(e) => set("label", e.target.value)} />
        </label>
        <label className="text-muted">Colour
          <input type="color" className="block h-[30px] w-12 rounded-md border border-line bg-surface"
            value={draft.colour ?? a.colour} onChange={(e) => set("colour", e.target.value.toUpperCase())} />
        </label>
      </div>
      <div className="mt-2 grid grid-cols-3 gap-2 text-sm">
        <label className="text-muted">Monthly budget ($)
          <input type="number" min={0} step="1" className={field} value={draft.monthly_budget_usd}
            onChange={(e) => set("monthly_budget_usd", Number(e.target.value))} />
        </label>
        <label className="text-muted">Amber at (%)
          <input type="number" min={1} className={field} value={draft.amber_pct}
            onChange={(e) => set("amber_pct", Number(e.target.value))} />
        </label>
        <label className="text-muted">Red at (%)
          <input type="number" min={1} className={field} value={draft.red_pct}
            onChange={(e) => set("red_pct", Number(e.target.value))} />
        </label>
      </div>
      <label className="mt-2 flex items-center gap-2 text-sm text-ink">
        <input type="checkbox" checked={draft.hidden} onChange={(e) => set("hidden", e.target.checked)} />
        Hide (left out of the picker and of All accounts)
      </label>

      <div className="mt-3 text-xs">
        <div className="text-muted mb-1">Config folders</div>
        <ul className="flex flex-col gap-1.5">
          {a.dirs.map((d) => (
            <li key={d.path} className="flex flex-wrap items-center gap-2">
              <span className="font-mono text-ink break-all">{d.path}</span>
              <span className="text-muted">{d.origin}</span>
              <select aria-label={`Account for ${d.path}`} disabled={busy}
                className="ml-auto border border-line rounded-md px-1 py-0.5 bg-surface text-ink"
                value={d.own_account === a.id ? "" : a.id}
                onChange={(e) => onMove(d.path, e.target.value || null)}>
                <option value="">Its own login</option>
                {all.filter((o) => o.id !== d.own_account).map((o) => (
                  <option key={o.id} value={o.id}>Counted in {o.label}</option>
                ))}
                {a.dirs.length > 1 && d.own_account !== `dir:${d.path}` && (
                  <option value={`dir:${d.path}`}>An account of its own</option>
                )}
              </select>
              {d.custom && (
                <button className="text-muted hover:text-ink" disabled={busy} onClick={() => onRemove(d.path)} title="Stop reading this folder">✕</button>
              )}
            </li>
          ))}
        </ul>
      </div>

      <div className="mt-3 flex items-center gap-2">
        <button onClick={() => saveMut.mutate()} disabled={edits === null || saveMut.isPending || problem !== null}
          className="px-3 py-1 rounded-md text-white shadow-glow disabled:opacity-50"
          style={{ background: "linear-gradient(135deg, var(--c1), var(--c3))" }}>
          {saveMut.isPending ? "Saving…" : "Save"}
        </button>
        {edits !== null && <button className={BUTTON} onClick={() => setEdits(null)}>Cancel</button>}
        {problem && <span className="text-xs text-red-500">{problem}</span>}
        {saveMut.error && <span className="text-xs text-red-500">{String(saveMut.error)}</span>}
      </div>
    </section>
  );
}
