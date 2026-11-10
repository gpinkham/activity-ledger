// The menu bar gauge's panel, opened from the ring in the menu bar. Presentation
// only: TrayApp loads the figures. Every figure is an estimate and says so.
import type { GaugeView, WorkSplit } from "../lib/types";
import { money, paceLine, rateNote, remainingLine, ringClass, ringWord, splitShares, words } from "../lib/gauge";

export interface TrayPanelProps {
  view: GaugeView | null;
  /** null while it is being worked out. */
  split: WorkSplit | null;
  error?: string | null;
  onOpen: () => void;
  /** Opens the main window's weekly report save flow. The panel writes nothing. */
  onSaveReport?: () => void;
  /** Quits the app. The icon has no menu of its own, so this is the only way out of the menu bar. */
  onQuit?: () => void;
}

const BUTTON = "px-3 py-1 rounded-md border border-line text-ink hover:bg-elevated";

/** Open and Quit, and Save weekly report once there are figures. Shown in every state,
 *  loading and error included: a click on the icon only ever opens this panel. */
function Actions({ onOpen, onSaveReport, onQuit }: Pick<TrayPanelProps, "onOpen" | "onSaveReport" | "onQuit">) {
  return (
    <div className="mt-1 flex flex-col gap-2">
      <button onClick={onOpen} className={BUTTON}>
        Open Activity Ledger
      </button>
      {onSaveReport && (
        <button onClick={onSaveReport} data-save-report className={BUTTON}>
          Save weekly report
        </button>
      )}
      {onQuit && (
        <button onClick={onQuit} data-quit className={BUTTON}>
          Quit Activity Ledger
        </button>
      )}
    </div>
  );
}

function Row({ label, value, className = "" }: { label: string; value: string; className?: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3 py-1">
      <span className="text-muted">{label}</span>
      <span className={`text-ink tabular-nums ${className}`}>{value}</span>
    </div>
  );
}

export function TrayPanel({ view, split, error, onOpen, onSaveReport, onQuit }: TrayPanelProps) {
  if (!view) {
    return (
      <div className="p-4 text-sm text-muted flex flex-col gap-2" data-state="loading">
        {error ? <p className="text-red-500">{error}</p> : <p>Reading this month&apos;s transcripts…</p>}
        <Actions onOpen={onOpen} onQuit={onQuit} />
      </div>
    );
  }
  const b = view.budget;
  const set = b.ring !== "none";
  const shares = split ? splitShares(split.work_usd, split.overhead_usd) : null;
  return (
    <div className="p-4 text-sm flex flex-col gap-2" data-state={b.ring}>
      <div>
        <div className="text-xs text-muted">Today</div>
        <div className="text-2xl font-semibold text-ink tabular-nums">
          {money(b.today)}
          {set && <span className="text-base font-normal text-muted"> of {money(b.daily_allowance)}</span>}
        </div>
        <div className={`text-xs ${ringClass(b.ring)}`} data-ring={b.ring}>
          {ringWord(b.ring)}
          {set && ` (${Math.round(b.today_share * 100)}%)`}
        </div>
      </div>

      <div className="border-t border-line pt-1">
        <Row label="Month to date" value={set ? `${money(b.month_to_date)} of ${money(b.monthly_budget)}` : money(b.month_to_date)} />
        {set && <Row label="Pace" value={paceLine(b)} />}
        <Row label="Projected month end" value={money(b.projected)}
          className={set && b.projected > b.monthly_budget ? "text-red-500" : ""} />
        {set && <Row label="Left per day" value={remainingLine(b)} />}
        <Row label="Today: work / overhead"
          value={split === null ? "working it out…" : shares ? `${shares.work}% / ${shares.overhead}%` : "no calls yet"} />
      </div>

      {!set && <p className="text-xs text-muted">Set a monthly budget in Activity Ledger, under Cost by activity, {words.Bar} budget.</p>}
      {error && <p className="text-xs text-red-500">{error}</p>}
      <p className="text-xs text-muted" data-note>{rateNote(view.rate_source)}</p>

      <Actions onOpen={onOpen} onSaveReport={onSaveReport} onQuit={onQuit} />
    </div>
  );
}
