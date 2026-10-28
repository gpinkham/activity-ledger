// The panel window under the menu bar ring (index.html#tray). It reloads its
// figures each time it is shown and once a minute while it is open.
import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getGauge, getGaugeSplit, openMainWindow } from "../lib/ipc";
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
    void load();
    const iv = setInterval(load, 60_000);
    const off = listen("gauge-shown", () => void load());
    return () => {
      clearInterval(iv);
      void off.then((f) => f());
    };
  }, [load]);

  return <TrayPanel view={view} split={split} error={error} onOpen={() => void openMainWindow()} />;
}
