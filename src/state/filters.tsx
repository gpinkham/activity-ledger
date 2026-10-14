import { createContext, useContext, useMemo, useState, type ReactNode } from "react";
import type { Filters } from "../lib/types";

interface FiltersCtx {
  filters: Filters;
  setFrom: (v?: string) => void;
  setTo: (v?: string) => void;
  setProject: (v?: string) => void;
}

const Ctx = createContext<FiltersCtx | null>(null);

export function FiltersProvider({ children }: { children: ReactNode }) {
  const [from, setFrom] = useState<string | undefined>();
  const [to, setTo] = useState<string | undefined>();
  const [project, setProject] = useState<string | undefined>();
  const value = useMemo<FiltersCtx>(
    () => ({ filters: { from, to, project }, setFrom, setTo, setProject }),
    [from, to, project],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useFilters() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useFilters must be used within FiltersProvider");
  return ctx;
}
