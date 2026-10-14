import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { refresh, getByProject } from "./lib/ipc";
import { useFilters } from "./state/filters";
import { useNav } from "./state/nav";
import { useTheme } from "./state/theme";
import { useQuery } from "@tanstack/react-query";
import Overview from "./screens/Overview";
import CostEfficiency from "./screens/CostEfficiency";
import Activity from "./screens/Activity";
import WorkflowScreen from "./screens/Workflow";
import Projects from "./screens/Projects";
import Sessions from "./screens/Sessions";
import SessionDetail from "./screens/SessionDetail";
import { ExportButton } from "./components/ExportButton";

const TABS: { id: import("./state/nav").Screen; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "cost", label: "Cost & efficiency" },
  { id: "activity", label: "Cost by activity" },
  { id: "workflow", label: "Workflow" },
  { id: "projects", label: "Projects" },
  { id: "sessions", label: "Sessions" },
];

export default function App() {
  const { screen, setScreen, sessionId } = useNav();
  const { filters, setFrom, setTo, setProject } = useFilters();
  const { theme, toggle } = useTheme();
  const qc = useQueryClient();
  const [refreshError, setRefreshError] = useState<string | null>(null);

  const projects = useQuery({ queryKey: ["projects-list"], queryFn: () => getByProject({}) });
  const refreshMut = useMutation({
    mutationFn: refresh,
    onSuccess: () => {
      qc.invalidateQueries();
      setRefreshError(null);
    },
    onError: (e) => setRefreshError(String(e)),
  });

  return (
    <div className="min-h-screen">
      <header className="sticky top-0 z-20 border-b border-line bg-bg/80 backdrop-blur px-4 py-3 flex items-center gap-4 flex-wrap">
        <h1 className="display text-lg text-ink flex items-center gap-2">
          <span
            className="inline-block h-4 w-4 rounded-sm"
            style={{ background: "linear-gradient(135deg, var(--c1), var(--c5))", boxShadow: "0 0 12px -2px var(--glow)" }}
          />
          Activity Ledger
        </h1>
        <nav className="flex gap-1">
          {TABS.map((t) => (
            <button
              key={t.id}
              onClick={() => setScreen(t.id)}
              className={`px-3 py-1.5 rounded-md text-sm transition-colors ${
                screen === t.id
                  ? "bg-elevated text-ink shadow-glow"
                  : "text-muted hover:text-ink hover:bg-elevated/60"
              }`}
            >
              {t.label}
            </button>
          ))}
        </nav>
        <div className="ml-auto flex items-center gap-2 text-sm text-ink">
          <input
            type="date"
            value={filters.from ?? ""}
            onChange={(e) => setFrom(e.target.value || undefined)}
            className="border border-line rounded-md px-2 py-1 bg-surface text-ink [color-scheme:inherit]"
          />
          <span className="text-muted">to</span>
          <input
            type="date"
            value={filters.to ?? ""}
            onChange={(e) => setTo(e.target.value || undefined)}
            className="border border-line rounded-md px-2 py-1 bg-surface text-ink [color-scheme:inherit]"
          />
          <select
            value={filters.project ?? ""}
            onChange={(e) => setProject(e.target.value || undefined)}
            className="border border-line rounded-md px-2 py-1 bg-surface text-ink max-w-[220px]"
          >
            <option value="">All projects</option>
            {(projects.data ?? []).map((p) => (
              <option key={p.project} value={p.project}>{p.project}</option>
            ))}
          </select>
          <ExportButton />
          <button
            onClick={() => refreshMut.mutate()}
            disabled={refreshMut.isPending}
            className="px-3 py-1 rounded-md text-white disabled:opacity-50 shadow-glow"
            style={{ background: "linear-gradient(135deg, var(--c1), var(--c3))" }}
          >
            {refreshMut.isPending ? "Refreshing…" : "Refresh"}
          </button>
          <button
            onClick={toggle}
            aria-label="Toggle theme"
            title={theme === "dark" ? "Switch to light" : "Switch to dark"}
            className="h-8 w-8 grid place-items-center rounded-md border border-line text-muted hover:text-ink hover:bg-elevated"
          >
            {theme === "dark" ? "☀" : "☾"}
          </button>
          {refreshError && <span className="text-xs text-red-500">{refreshError}</span>}
        </div>
      </header>
      <main className="p-4">
        {sessionId ? <SessionDetail sessionId={sessionId} /> : <ScreenBody screen={screen} />}
      </main>
    </div>
  );
}

function ScreenBody({ screen }: { screen: import("./state/nav").Screen }) {
  switch (screen) {
    case "overview": return <Overview />;
    case "cost": return <CostEfficiency />;
    case "activity": return <Activity />;
    case "workflow": return <WorkflowScreen />;
    case "projects": return <Projects />;
    case "sessions": return <Sessions />;
  }
}
