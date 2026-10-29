import { useEffect, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { getSettings, saveExport, setSettings, weeklyReport } from "../lib/ipc";
import { REPORT_WEEK_LABELS, hideProjectsOf, reportFileName, weekStart, withHideProjects, type ReportWeek } from "../lib/report";
import type { AppSettings } from "../lib/types";

/** Saves a week's Markdown report where the user picks. Written only when asked. */
export function WeeklyReportButton() {
  const [week, setWeek] = useState<ReportWeek>("last");
  const [settings, setLocalSettings] = useState<AppSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const hide = hideProjectsOf(settings);

  useEffect(() => {
    getSettings().then(setLocalSettings).catch(() => {});
  }, []);

  const onHide = async (next: boolean) => {
    if (!settings) return;
    const updated = withHideProjects(settings, next);
    setLocalSettings(updated);
    try {
      await setSettings(updated);
    } catch (e) {
      setError(String(e));
    }
  };

  const run = async (which: ReportWeek) => {
    setBusy(true);
    setError(null);
    try {
      const monday = weekStart(which, new Date());
      const md = await weeklyReport(monday, hide);
      const path = await save({
        defaultPath: reportFileName(monday),
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (path) await saveExport(path, md);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // The tray's "Save weekly report" opens this window and asks for last week.
  const runRef = useRef(run);
  runRef.current = run;
  useEffect(() => {
    const off = listen("start-weekly-report", () => void runRef.current("last"));
    return () => {
      off.then((f) => f()).catch(() => {});
    };
  }, []);

  return (
    <span className="flex items-center gap-2">
      <select
        value={week}
        onChange={(e) => setWeek(e.target.value as ReportWeek)}
        aria-label="Report week"
        className="border border-line rounded-md px-2 py-1 bg-surface text-ink"
      >
        {(Object.keys(REPORT_WEEK_LABELS) as ReportWeek[]).map((w) => (
          <option key={w} value={w}>{REPORT_WEEK_LABELS[w]}</option>
        ))}
      </select>
      <label className="flex items-center gap-1 text-xs text-muted">
        <input type="checkbox" checked={hide} disabled={!settings} onChange={(e) => void onHide(e.target.checked)} />
        Hide project names
      </label>
      <button onClick={() => void run(week)} disabled={busy} className="px-3 py-1 rounded border border-neutral-300 dark:border-neutral-700 disabled:opacity-50">
        {busy ? "Saving…" : "Weekly report"}
      </button>
      {error && <span className="text-xs text-red-600">{error}</span>}
    </span>
  );
}
