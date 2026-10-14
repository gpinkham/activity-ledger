import { createContext, useContext, useMemo, useState, type ReactNode } from "react";

export type Screen = "overview" | "cost" | "activity" | "workflow" | "projects" | "sessions";

interface Nav {
  screen: Screen;
  sessionId: string | null;
  sessionsProject: string | null;
  setScreen: (s: Screen) => void;
  openSessionsForProject: (project: string) => void;
  openSession: (id: string) => void;
  back: () => void;
}

const Ctx = createContext<Nav | null>(null);

export function NavProvider({ children }: { children: ReactNode }) {
  const [screen, setScreenState] = useState<Screen>("overview");
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [sessionsProject, setSessionsProject] = useState<string | null>(null);

  const value = useMemo<Nav>(
    () => ({
      screen,
      sessionId,
      sessionsProject,
      setScreen: (s) => { setSessionId(null); setSessionsProject(null); setScreenState(s); },
      openSessionsForProject: (project) => { setSessionId(null); setSessionsProject(project); setScreenState("sessions"); },
      openSession: (id) => setSessionId(id),
      back: () => setSessionId(null),
    }),
    [screen, sessionId, sessionsProject],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useNav() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useNav must be used within NavProvider");
  return ctx;
}
