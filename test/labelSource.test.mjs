// Tests for src/lib/labelSource.ts (the Cost by activity label toggle). Run with
// `npm run test:ui`. The TypeScript is compiled with the esbuild Vite already uses.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { transformSync } from "esbuild";

const src = readFileSync(new URL("../src/lib/labelSource.ts", import.meta.url), "utf8");
const out = join(mkdtempSync(join(tmpdir(), "ci-ui-")), "labelSource.mjs");
writeFileSync(out, transformSync(src, { loader: "ts", format: "esm" }).code);
const L = await import(out);

function memoryStorage(init = {}) {
  const m = new Map(Object.entries(init));
  return { getItem: (k) => (m.has(k) ? m.get(k) : null), setItem: (k, v) => m.set(k, String(v)), m };
}
const broken = { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("blocked"); } };

test("the choice is remembered, and anything else reads as the rule labels", () => {
  const s = memoryStorage();
  assert.equal(L.readSavedSource(s), "rule");
  L.saveSource(s, "model");
  assert.equal(s.m.get(L.STORAGE_KEY), "model");
  assert.equal(L.readSavedSource(s), "model");
  assert.equal(L.readSavedSource(memoryStorage({ [L.STORAGE_KEY]: "llm" })), "rule");
  assert.equal(L.readSavedSource(broken), "rule");
  assert.doesNotThrow(() => L.saveSource(broken, "model"));
  assert.equal(L.readSavedSource(undefined), "rule");
});

test("the LLM view is used only while LLM labels exist", () => {
  assert.equal(L.effectiveSource("model", true), "model");
  assert.equal(L.effectiveSource("model", null), "model", "unknown yet: ask for it, the report says");
  assert.equal(L.effectiveSource("model", false), "rule");
  assert.equal(L.effectiveSource("rule", true), "rule");
  assert.equal(L.hasModelLabels({ labelled_segments: 0 }), false);
  assert.equal(L.hasModelLabels({ labelled_segments: 3 }), true);
});

test("coverage says how much the LLM labelled, in segments, calls and cost", () => {
  const c = { model: "qwen3:8b", segments: 200, labelled_segments: 150, calls: 1000, labelled_calls: 700, cost_usd: 40, labelled_usd: 30 };
  const t = L.coverageText(c, (n) => `$${n.toFixed(2)}`);
  assert.match(t, /qwen3:8b/);
  assert.match(t, /150 of 200 segments \(75%\)/);
  assert.match(t, /700 of 1,000 calls/);
  assert.match(t, /\$30\.00 of \$40\.00/);
  assert.match(t, /50 segments/, "the unlabelled count is spelled out");
  assert.match(L.coverageText({ ...c, labelled_segments: 200, labelled_calls: 1000, labelled_usd: 40 }, String), /every segment/);
  assert.equal(L.coverageText({ ...c, labelled_segments: 0, labelled_calls: 0, labelled_usd: 0 }, String), null);
  assert.doesNotMatch(t, /[\u2013\u2014]/);
});

test("each day is split into work, overhead and not labelled", () => {
  const rows = [
    { activity: "coding", group: "work" },
    { activity: "overhead", group: "overhead" },
    { activity: "not labelled by the model", group: "unlabelled" },
  ];
  const days = L.groupByDay(rows, [
    { day: "d1", activity: "coding", cost_usd: 1 },
    { day: "d1", activity: "overhead", cost_usd: 2 },
    { day: "d1", activity: "not labelled by the model", cost_usd: 4 },
    { day: "d2", activity: "coding", cost_usd: 8 },
  ]);
  assert.deepEqual(days, [
    { day: "d1", work: 1, overhead: 2, unlabelled: 4 },
    { day: "d2", work: 8, overhead: 0, unlabelled: 0 },
  ]);
});

test("in the LLM view a share is of the labelled cost, so overhead does not look smaller", () => {
  // $100 total, $75 not labelled, $10 of the labelled $25 is overhead: 40%, not 10%.
  assert.equal(L.shareOfLabelled(10, 100, 75), 0.4);
  assert.equal(L.shareOfLabelled(5, 100, 100), 0, "nothing labelled: no share, not a division by zero");
  assert.equal(L.shareOfLabelled(5, 100, 0), 0.05);
});

test("the coverage percent never rounds up to 100% while segments are left", () => {
  const c = { model: "m", segments: 1000, labelled_segments: 995, calls: 1000, labelled_calls: 995, cost_usd: 1, labelled_usd: 1 };
  assert.match(L.coverageText(c, String), /\(99%\)/);
});
