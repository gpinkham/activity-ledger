import { createContext, useContext, useMemo, useState, type ReactNode } from "react";
import type { Filters } from "../lib/types";

interface FiltersCtx {
  filters: Filters;
  setFrom: (v?: string) => void;
  setTo: (v?: string) => void;
  setProject: (v?: string) => void;
  /** One Claude account, or undefined for every account. Remembered across launches. */
  setAccount: (v?: string) => void;
}

const Ctx = createContext<FiltersCtx | null>(null);

const ACCOUNT_KEY = "activity-ledger.account";

/** The account picked last time. Storage can be missing or refuse (a private
 *  webview, tests): then every account. */
function savedAccount(): string | undefined {
  try {
    return globalThis.localStorage?.getItem(ACCOUNT_KEY) || undefined;
  } catch {
    return undefined;
  }
}

function saveAccount(v?: string) {
  try {
    if (v) globalThis.localStorage?.setItem(ACCOUNT_KEY, v);
    else globalThis.localStorage?.removeItem(ACCOUNT_KEY);
  } catch {
    // Not remembered; the choice still holds for this run.
  }
}

export function FiltersProvider({ children }: { children: ReactNode }) {
  const [from, setFrom] = useState<string | undefined>();
  const [to, setTo] = useState<string | undefined>();
  const [project, setProject] = useState<string | undefined>();
  const [account, setAccountState] = useState<string | undefined>(savedAccount);
  const value = useMemo<FiltersCtx>(
    () => ({
      filters: { from, to, project, account },
      setFrom,
      setTo,
      setProject,
      setAccount: (v?: string) => {
        saveAccount(v);
        setAccountState(v);
        // A project picked in one account may not exist in the next.
        setProject(undefined);
      },
    }),
    [from, to, project, account],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useFilters() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useFilters must be used within FiltersProvider");
  return ctx;
}
