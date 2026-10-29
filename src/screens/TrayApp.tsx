// The panel window under the menu bar ring (index.html#tray). It reloads its
// figures each time it is shown and once a minute while it is open, and does
// nothing while it is hidden (tray.rs sends gauge-shown and gauge-hidden).
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { panelPoller } from "../lib/gauge";
import { getGauge, getGaugeSplit, openMainWindow, openWeeklyReport } from "../lib/ipc";
import type { GaugeView, WorkSplit } from "../lib/types";
import { TrayPanel } from "./TrayPanel";

export default function TrayApp() {
  const [view, setView] = useState<GaugeView | null>(null);
  const [split, setSplit] = useState<WorkSplit | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await getGauge());
      setError(null);
      // The split brings the main cache up to date first, so it comes second; the
      // last one stays on screen until it arrives.
      setSplit(await getGaugeSplit());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    // The window is created hidden and shown straight away, before these listeners
    // exist, so the first show is taken as given.
    const poll = panelPoller(() => void load(), 60_000);
    poll.shown();
    const offs = [listen("gauge-shown", poll.shown), listen("gauge-hidden", poll.hidden)];
    return () => {
      poll.hidden();
      for (const off of offs) void off.then((f) => f());
    };
  }, [load]);

  return <TrayPanel view={view} split={split} error={error} onOpen={() => void openMainWindow()} onSaveReport={() => void openWeeklyReport()} />;
}
