// Claude accounts as the window shows them: which to offer, how to name them, and
// when rows need an account chip. Plain functions, tested in test/accounts.test.mjs.
import type { Account } from "./types";

/** The accounts offered in pickers and counted in "All accounts". */
export const visibleAccounts = (accounts: Account[] | undefined): Account[] =>
  (accounts ?? []).filter((a) => !a.settings.hidden);

export const accountById = (accounts: Account[] | undefined, id: string | null | undefined): Account | undefined =>
  id ? accounts?.find((a) => a.id === id) : undefined;

/** The picked account, while it still exists and is not hidden; else every account.
 *  Before the accounts load, the pick is kept as it is. */
export function pickedAccount(accounts: Account[] | undefined, id: string | undefined): string | undefined {
  if (!id || accounts === undefined) return id;
  return visibleAccounts(accounts).some((a) => a.id === id) ? id : undefined;
}

/** Rows say whose they are when they can be from more than one account. */
export const showAccountChips = (accounts: Account[] | undefined, picked: string | undefined): boolean =>
  !picked && visibleAccounts(accounts).length > 1;

/** "Enterprise · ana@acme.com": the plan and the login, as known. */
export const accountDetail = (a: Account): string => [a.plan, a.email].filter(Boolean).join(" · ");

/** Where an account's transcripts come from: "Windows + WSL · Ubuntu". */
export const accountOrigins = (a: Account): string => [...new Set(a.dirs.map((d) => d.origin))].join(" + ");

/** A `#RRGGBB` colour, or a reason it is not one. */
export function checkColour(c: string): string | null {
  return /^#[0-9a-fA-F]{6}$/.test(c) ? null : "A colour is # and six hex digits, like #3B82F6.";
}
