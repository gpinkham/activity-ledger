// The weekly report's week choices, file name and hide setting. Plain functions,
// tested in test/report.test.mjs. The report itself is built in Rust (report.rs).
import type { AppSettings } from "./types";

export type ReportWeek = "last" | "this";

export const REPORT_WEEK_LABELS: Record<ReportWeek, string> = {
  last: "Last week",
  this: "This week so far",
};

const pad = (n: number) => String(n).padStart(2, "0");
const ymd = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

/** The local Monday, YYYY-MM-DD, of the week holding `today`, or of the week before. */
export function weekStart(week: ReportWeek, today: Date): string {
  const d = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  const sinceMonday = (d.getDay() + 6) % 7;
  d.setDate(d.getDate() - sinceMonday - (week === "last" ? 7 : 0));
  return ymd(d);
}

/** ISO 8601 week of a date given as YYYY-MM-DD: [ISO year, week number]. */
export function isoWeek(dateStr: string): [number, number] {
  const [y, m, d] = dateStr.split("-").map(Number);
  // Work in UTC so a daylight saving change cannot move the day.
  const t = new Date(Date.UTC(y, m - 1, d));
  const dayNum = (t.getUTCDay() + 6) % 7; // Monday 0
  t.setUTCDate(t.getUTCDate() - dayNum + 3); // this week's Thursday
  const isoYear = t.getUTCFullYear();
  const firstThursday = new Date(Date.UTC(isoYear, 0, 4));
  firstThursday.setUTCDate(firstThursday.getUTCDate() - ((firstThursday.getUTCDay() + 6) % 7) + 3);
  const week = 1 + Math.round((t.getTime() - firstThursday.getTime()) / (7 * 86_400_000));
  return [isoYear, week];
}

/** "activity-ledger-2026-W44.md" for the week starting on `monday`. */
export function reportFileName(monday: string): string {
  const [year, week] = isoWeek(monday);
  return `activity-ledger-${year}-W${pad(week)}.md`;
}

/** The saved hide setting; a settings file from before 0.3 has none, so off. */
export function hideProjectsOf(settings: Partial<AppSettings> | null | undefined): boolean {
  return settings?.report_hide_projects === true;
}

/** Settings with the hide choice changed and everything else kept. */
export function withHideProjects(settings: AppSettings, hide: boolean): AppSettings {
  return { ...settings, report_hide_projects: hide };
}
