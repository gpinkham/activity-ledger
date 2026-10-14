import { useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { exportBundle, saveExport } from "../lib/ipc";
import { useFilters } from "../state/filters";

export function ExportButton() {
  const { filters } = useFilters();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onExport = async () => {
    setBusy(true);
    setError(null);
    try {
      const json = await exportBundle(filters);
      const path = await save({
        defaultPath: "activity-ledger-metrics.json",
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (path) await saveExport(path, json);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <span className="flex items-center gap-2">
      <button onClick={onExport} disabled={busy} className="px-3 py-1 rounded border border-neutral-300 dark:border-neutral-700 disabled:opacity-50">
        {busy ? "Exporting…" : "Export metrics"}
      </button>
      {error && <span className="text-xs text-red-600">{error}</span>}
    </span>
  );
}
