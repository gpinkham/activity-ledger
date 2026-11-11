// The Claude account picker in the header, and the chip that says whose a row is.
import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { getAccounts } from "../lib/ipc";
import { accountById, accountDetail, pickedAccount, visibleAccounts } from "../lib/accounts";
import { useFilters } from "../state/filters";
import type { Account } from "../lib/types";

export function useAccounts() {
  return useQuery({ queryKey: ["accounts"], queryFn: getAccounts });
}

export function AccountDot({ colour, size = 8 }: { colour: string; size?: number }) {
  return <span aria-hidden className="inline-block rounded-full shrink-0" style={{ background: colour, width: size, height: size }} />;
}

/** A row's account: its colour and name. */
export function AccountChip({ id, accounts }: { id: string; accounts: Account[] | undefined }) {
  const a = accountById(accounts, id);
  if (!a) return null;
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-line px-1.5 py-px text-[11px] text-muted whitespace-nowrap"
      title={accountDetail(a) || a.label} data-account={a.id}>
      <AccountDot colour={a.colour} size={6} />
      {a.label}
    </span>
  );
}

/** "All accounts", then each account not hidden. Hidden with only one account. */
export function AccountSelect() {
  const { filters, setAccount } = useFilters();
  const accounts = useAccounts();
  const visible = visibleAccounts(accounts.data);
  // An account hidden or gone since it was picked: back to every account.
  const picked = pickedAccount(accounts.data, filters.account);
  useEffect(() => {
    if (picked !== filters.account) setAccount(picked);
  }, [picked, filters.account, setAccount]);

  if (visible.length < 2 && !filters.account) return null;
  const current = accountById(accounts.data, picked);
  return (
    <span className="inline-flex items-center gap-1.5 border border-line rounded-md pl-2 bg-surface">
      <AccountDot colour={current?.colour ?? "var(--muted, #8E8E93)"} />
      <select
        aria-label="Claude account"
        value={picked ?? ""}
        onChange={(e) => setAccount(e.target.value || undefined)}
        className="py-1 pr-2 bg-surface text-ink rounded-md max-w-[200px] outline-none"
        title={current ? accountDetail(current) : "Every Claude account on this machine"}
      >
        <option value="">All accounts</option>
        {visible.map((a) => (
          <option key={a.id} value={a.id}>{a.label}{a.plan ? ` (${a.plan})` : ""}</option>
        ))}
      </select>
    </span>
  );
}
