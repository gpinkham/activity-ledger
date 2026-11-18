// Wording and checks for the menu bar budget gauge (TrayPanel, BudgetCard). Plain
// functions, tested in test/gauge.test.mjs.
import type { BudgetView, GaugeView, Jump, Ring } from "./types";

export const money = (n: number) => `$${n.toFixed(2)}`;

/** The words for where the ring lives, per platform. Add a row for a new platform. */
export type Platform = "macos" | "windows" | "linux";
export const PLATFORM_WORDS: Record<Platform, { machine: string; bar: string; Bar: string; dock: string }> = {
  macos: { machine: "Mac", bar: "menu bar", Bar: "Menu bar", dock: "Dock" },
  windows: { machine: "PC", bar: "system tray", Bar: "System tray", dock: "taskbar" },
  linux: { machine: "computer", bar: "system tray", Bar: "System tray", dock: "taskbar" },
};

/** This platform, from the webview's user agent; macOS when it cannot tell (as in the tests). */
export function platformOf(userAgent: string): Platform {
  if (/Windows/.test(userAgent)) return "windows";
  if (/Linux|X11/.test(userAgent) && !/Android/.test(userAgent)) return "linux";
  return "macos";
}
export const words = PLATFORM_WORDS[platformOf(typeof navigator !== "undefined" ? navigator.userAgent : "")];

/** The line under every figure: what the numbers are, and what they leave out. */
export function rateNote(source: GaugeView["rate_source"]): string {
  const rates = source === "configured" ? "at configured rates" : "at list price";
  return `Estimate ${rates}, this ${words.machine} only, Claude Code only.`;
}

export function ringWord(ring: Ring): string {
  return {
    none: "No budget set", green: "Within today's allowance", amber: "Near today's allowance", red: "Over today's allowance",
    weekend: "Weekend: counted in the month",
  }[ring];
}

/** What today is measured against: what is left, or the even spread from an older backend. */
export const todayAllowance = (b: BudgetView): number => b.today_allowance ?? b.daily_allowance;

/** The ring's words under today's spend. "Within today's allowance (42%)", or with
 *  the budget used up before today, "Month budget used: $315.00 of $300.00" and no share. */
export function ringLine(b: BudgetView): string {
  if (b.used_up) return `Month budget used: ${money(b.month_to_date)} of ${money(b.monthly_budget)}`;
  if (b.ring === "none" || b.ring === "weekend") return ringWord(b.ring);
  return `${ringWord(b.ring)} (${Math.round(b.today_share * 100)}%)`;
}

/** The grey note under today's allowance when it differs from the even spread by 5%
 *  or more: "Even spread $9.68, lowered by the month so far". Otherwise null, so a
 *  normal month shows one number. */
export function allowanceNote(b: BudgetView): string | null {
  if (b.ring === "none" || b.ring === "weekend" || b.used_up || b.daily_allowance <= 0) return null;
  const today = todayAllowance(b);
  if (Math.abs(today - b.daily_allowance) / b.daily_allowance < 0.05) return null;
  return `Even spread ${money(b.daily_allowance)}, ${today < b.daily_allowance ? "lowered" : "raised"} by the month so far`;
}

/** Text colour for a ring state. */
export function ringClass(ring: Ring): string {
  return { none: "text-muted", green: "text-emerald-500", amber: "text-amber-500", red: "text-red-500", weekend: "text-muted" }[ring];
}

/** "$4.27 ahead of an even pace", "$3.10 under an even pace", or "On an even pace". */
export function paceLine(b: BudgetView): string {
  const d = b.month_to_date - b.on_pace;
  if (Math.abs(d) < 0.005) return "On an even pace";
  return d > 0 ? `${money(d)} ahead of an even pace` : `${money(-d)} under an even pace`;
}

/** The jump line's words. "Today is 2.1 times a usual day ($84.00 so far, usually
 *  about $40.00)." or "Up for 3 days: about 1.8 times the week before." */
export function jumpLine(j: Jump): string {
  const times = j.times.toFixed(1);
  const line = j.kind === "today"
    ? `Today is ${times} times a usual day (${money(j.spent)} so far, usually about ${money(j.usual)}).`
    : `Up for 3 days: about ${times} times the week before (${money(j.spent)} a day, usually about ${money(j.usual)}).`;
  // The backend already left the home folder out of the name (jump.rs, project_label).
  return j.project ? `${line} Mostly from ${j.project}.` : line;
}

export function remainingLine(b: BudgetView): string {
  if (b.weekdays_only) {
    if (b.days_left === 0) return "No weekdays left this month";
    if (b.month_to_date >= b.monthly_budget) return "Nothing left this month";
    if (b.days_left === 1) return `${money(b.remaining_per_day)} for ${b.ring === "weekend" ? "the last weekday" : "today"}`;
    return `${money(b.remaining_per_day)} for each of the ${b.days_left} weekdays left`;
  }
  if (b.month_to_date >= b.monthly_budget) return "Nothing left this month";
  const days = b.days_left === 1 ? "today" : `each of the ${b.days_left} days left`;
  return `${money(b.remaining_per_day)} for ${days}`;
}

/** The projected month end, or "No weekday yet" before a weekdays-only month's first weekday. */
export function projectedLine(b: BudgetView): string {
  return b.no_weekday_yet ? "No weekday yet" : money(b.projected);
}

/** Under the figures on a weekdays-only budget: what the day counts mean. */
export function weekdaysNote(b: BudgetView): string | null {
  if (!b.weekdays_only) return null;
  return `Spread over the ${b.days_in_month} weekdays this month, Monday to Friday. Weekend spend counts in the month.`;
}

/** Real work and overhead as whole percentages of today's spend. */
export function splitShares(work: number, overhead: number): { work: number; overhead: number } | null {
  const all = work + overhead;
  if (all <= 0) return null;
  const w = Math.round((work / all) * 100);
  return { work: w, overhead: 100 - w };
}

/** A budget form's values, checked: the budget is dollars, 0 for none; amber is
 *  below red; both are percentages of today's allowance. */
export function checkBudget(budget: number, amber: number, red: number): string | null {
  if (!Number.isFinite(budget) || budget < 0) return "The budget is a number of dollars, 0 for none.";
  if (!Number.isFinite(amber) || !Number.isFinite(red) || amber <= 0 || red <= 0) return "Thresholds are percentages above 0.";
  if (amber >= red) return "Amber must be below red.";
  if (red > 1000) return "Red is a percentage of today's allowance: 1000 at most.";
  return null;
}

/** What the budget card shows: what was typed, else the saved settings. Settings
 *  fetched again do not reset what was typed. */
export const cardDraft = <T>(edits: T | null, fetched: T | undefined): T | undefined => edits ?? fetched;

/**
 * The panel's refresh while it is on screen: load at once, then once a minute, and
 * nothing at all while it is hidden. The panel window is hidden, never closed, so a
 * timer left running would refresh the whole store every minute for nobody.
 */
export function panelPoller(
  load: () => void,
  every: number,
  timers: { set: (f: () => void, ms: number) => unknown; clear: (id: unknown) => void } = {
    set: (f, ms) => setInterval(f, ms),
    clear: (id) => clearInterval(id as ReturnType<typeof setInterval>),
  },
) {
  let id: unknown = null;
  const hidden = () => {
    if (id !== null) timers.clear(id);
    id = null;
  };
  return {
    shown() {
      hidden();
      load();
      id = timers.set(load, every);
    },
    hidden,
    get running() {
      return id !== null;
    },
  };
}
