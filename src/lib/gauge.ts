// Wording and checks for the menu bar budget gauge (TrayPanel, BudgetCard). Plain
// functions, tested in test/gauge.test.mjs.
import type { BudgetView, GaugeView, Ring } from "./types";

export const money = (n: number) => `$${n.toFixed(2)}`;

/** The line under every figure: what the numbers are, and what they leave out. */
export function rateNote(source: GaugeView["rate_source"]): string {
  const rates = source === "configured" ? "at configured rates" : "at list price";
  return `Estimate ${rates}, this Mac only, Claude Code only.`;
}

export function ringWord(ring: Ring): string {
  return { none: "No budget set", green: "Within today's allowance", amber: "Near today's allowance", red: "Over today's allowance" }[ring];
}

/** Text colour for a ring state. */
export function ringClass(ring: Ring): string {
  return { none: "text-muted", green: "text-emerald-500", amber: "text-amber-500", red: "text-red-500" }[ring];
}

/** "$4.27 ahead of an even pace", "$3.10 under an even pace", or "On an even pace". */
export function paceLine(b: BudgetView): string {
  const d = b.month_to_date - b.on_pace;
  if (Math.abs(d) < 0.005) return "On an even pace";
  return d > 0 ? `${money(d)} ahead of an even pace` : `${money(-d)} under an even pace`;
}

export function remainingLine(b: BudgetView): string {
  const days = b.days_left === 1 ? "today" : `each of the ${b.days_left} days left`;
  return `${money(b.remaining_per_day)} for ${days}`;
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
