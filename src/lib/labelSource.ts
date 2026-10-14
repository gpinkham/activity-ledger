// The Cost by activity label toggle: rule labels or the local model's (LLM) labels.
// Pure, so test/labelSource.test.mjs can run it without a browser.

export type LabelSource = "rule" | "model";
export type Group = "overhead" | "work" | "unlabelled";

export interface LabelCoverage {
  model: string; segments: number; labelled_segments: number;
  calls: number; labelled_calls: number; cost_usd: number; labelled_usd: number;
}

export const STORAGE_KEY = "ci-activity-labels";

type KV = Pick<Storage, "getItem" | "setItem">;

/** The saved choice; the rule labels when none is saved or storage is unavailable. */
export function readSavedSource(storage: KV | undefined): LabelSource {
  try {
    return storage?.getItem(STORAGE_KEY) === "model" ? "model" : "rule";
  } catch {
    return "rule";
  }
}

export function saveSource(storage: KV | undefined, s: LabelSource): void {
  try {
    storage?.setItem(STORAGE_KEY, s);
  } catch {
    /* ignore */
  }
}

export const hasModelLabels = (c: Pick<LabelCoverage, "labelled_segments">) => c.labelled_segments > 0;

/** The view to show: the LLM view only while LLM labels exist (null = not known yet). */
export function effectiveSource(want: LabelSource, hasLabels: boolean | null): LabelSource {
  return want === "model" && hasLabels === false ? "rule" : want;
}

const n = (x: number) => x.toLocaleString("en-US");
// Floor, so a range with segments left never reads 100%.
const share = (part: number, whole: number) => (whole > 0 ? Math.floor((part / whole) * 100) : 0);

/** One line on how much the LLM labelled; null when it labelled nothing. */
export function coverageText(c: LabelCoverage, usd: (v: number) => string): string | null {
  if (c.labelled_segments === 0) return null;
  const who = c.model ? `LLM labels (${c.model})` : "LLM labels";
  if (c.labelled_segments >= c.segments) return `${who} cover every segment in this range.`;
  const left = c.segments - c.labelled_segments;
  return `${who} cover ${n(c.labelled_segments)} of ${n(c.segments)} segments (${share(c.labelled_segments, c.segments)}%), `
    + `${n(c.labelled_calls)} of ${n(c.calls)} calls, ${usd(c.labelled_usd)} of ${usd(c.cost_usd)}. `
    + `The other ${n(left)} segments are counted under "not labelled by the model", so the totals still add up.`;
}

/** `part` as a share of the labelled cost (`whole` minus what is not labelled), 0..1.
 *  In the LLM view, unlabelled cost is neither work nor overhead, so it is left out
 *  of the denominator; otherwise low coverage would make overhead look smaller. */
export function shareOfLabelled(part: number, whole: number, unlabelled: number): number {
  const labelled = whole - unlabelled;
  return labelled > 0 ? part / labelled : 0;
}

export interface DayGroups { day: string; work: number; overhead: number; unlabelled: number }

/** Cost per day split by the group of each row's label. */
export function groupByDay(
  rows: { activity: string; group: Group }[],
  byDay: { day: string; activity: string; cost_usd: number }[],
): DayGroups[] {
  const groupOf = new Map(rows.map((x) => [x.activity, x.group]));
  const days = new Map<string, DayGroups>();
  for (const d of byDay) {
    const e = days.get(d.day) ?? { day: d.day, work: 0, overhead: 0, unlabelled: 0 };
    e[groupOf.get(d.activity) ?? "work"] += d.cost_usd;
    days.set(d.day, e);
  }
  return [...days.values()];
}
