// The panel window under the menu bar ring (index.html#tray). It reloads its
// figures each time it is shown and once a minute while it is open, and does
// nothing while it is hidden (tray.rs sends gauge-shown and gauge-hidden).
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { panelPoller } from "../lib/gauge";
import { visibleAccounts } from "../lib/accounts";
import { fitPanel, getAccounts, getGauge, getGaugeSplit, openMainWindow, openWeeklyReport, quitApp, setTrayAccount } from "../lib/ipc";
import type { GaugeView, WorkSplit } from "../lib/types";
import { TrayPanel, type PanelAccount } from "./TrayPanel";

export default function TrayApp() {
  const [view, setView] = useState<GaugeView | null>(null);
  const [split, setSplit] = useState<WorkSplit | null>(null);
  const [accounts, setAccounts] = useState<PanelAccount[]>([]);
  const [error, setError] = useState<string | null>(null);
  const box = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    try {
      setView(await getGauge());
      setError(null);
      setAccounts(visibleAccounts(await getAccounts()).map(({ id, label, colour }) => ({ id, label, colour })));
      // The split brings the main cache up to date first, so it comes last; the
      // last one stays on screen until it arrives.
      setSplit(await getGaugeSplit());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // The ring follows the account picked here; the main window's budget card shows it too.
  const pick = useCallback(async (id: string | null) => {
    try {
      await setTrayAccount(id);
      setSplit(null);
      await load();
    } catch (e) {
      setError(String(e));
    }
  }, [load]);

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

  // The window takes the height of what it shows, so a row or button added later,
  // a wrapped line or the loading state never leaves a scrollbar or empty space.
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    let last = 0;
    const fit = () => {
      const h = Math.ceil(el.getBoundingClientRect().height);
      if (h > 0 && h !== last) {
        last = h;
        void fitPanel(h);
      }
    };
    const watch = new ResizeObserver(fit);
    watch.observe(el);
    fit();
    return () => watch.disconnect();
  }, []);

  return (
    <div ref={box}>
      <TrayPanel view={view} split={split} error={error} accounts={accounts} onPickAccount={(id) => void pick(id)}
        onOpen={() => void openMainWindow()} onSaveReport={() => void openWeeklyReport()} onQuit={() => void quitApp()} />
    </div>
  );
}
