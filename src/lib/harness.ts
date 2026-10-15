// The trigger file card and the By trigger table: what an agent harness started.
import type { TriggerRow, TriggerStatus } from "./types";

/** "Found a <name> folder at <root>. Turn it on?" for the first adapter that is
 *  off but whose harness folder is on this machine; null otherwise. */
export function foundHint(status: TriggerStatus): string | null {
  const a = status.adapters.find((x) => !x.enabled && x.found);
  return a ? `Found a ${a.name} folder at ${a.root ?? "its folder"}. Turn it on?` : null;
}

/** How much of a trigger's cost was the work itself, 0..1. */
export function workShare(row: TriggerRow): number {
  return row.cost_usd > 0 ? row.work_usd / row.cost_usd : 0;
}

/** Only typed prompts: the table would say nothing. */
export function showTriggers(rows: TriggerRow[]): boolean {
  return rows.some((r) => r.trigger !== "typed prompt");
}
