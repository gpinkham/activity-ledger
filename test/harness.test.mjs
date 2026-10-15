// Tests for src/lib/harness.ts (the trigger file card and the By trigger table).
// Run with `npm run test:ui`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { transformSync } from "esbuild";

const src = readFileSync(new URL("../src/lib/harness.ts", import.meta.url), "utf8");
const out = join(mkdtempSync(join(tmpdir(), "al-ui-")), "harness.mjs");
writeFileSync(out, transformSync(src, { loader: "ts", format: "esm" }).code);
const H = await import(out);

const md = (enabled, found) => ({ name: "Munder Difflin", enabled, root: "~/HarnessAgents/hive", found });

test("a harness folder found while its adapter is off gets one hint", () => {
  assert.equal(H.foundHint({ path: "p", error: null, adapters: [md(false, true)] }),
    "Found a Munder Difflin folder at ~/HarnessAgents/hive. Turn it on?");
  assert.equal(H.foundHint({ path: "p", error: null, adapters: [md(false, false)] }), null, "nothing found");
  assert.equal(H.foundHint({ path: "p", error: null, adapters: [md(true, false)] }), null, "already on");
  assert.equal(H.foundHint({ path: "p", error: null, adapters: [md(true, true)] }), null, "on and found: no hint");
  assert.equal(H.foundHint({ path: "p", error: null, adapters: [] }), null);
});

test("the work share of a trigger row, and a row with no cost", () => {
  assert.equal(H.workShare({ trigger: "inbox handling", segments: 2, cost_usd: 4, work_usd: 1 }), 0.25);
  assert.equal(H.workShare({ trigger: "x", segments: 0, cost_usd: 0, work_usd: 0 }), 0);
});

test("the table is worth showing only when something other than typed prompts started work", () => {
  const typed = { trigger: "typed prompt", segments: 3, cost_usd: 3, work_usd: 3 };
  assert.equal(H.showTriggers([typed]), false);
  assert.equal(H.showTriggers([typed, { trigger: "compaction", segments: 1, cost_usd: 1, work_usd: 0 }]), true);
  assert.equal(H.showTriggers([]), false);
});

test("the kind of work chart leaves out overhead and rows the model has not labelled", () => {
  const rows = [{ activity: "coding", group: "work" }, { activity: "standup", group: "overhead" },
    { activity: "not labelled by the model", group: "unlabelled" }, { activity: "research", group: "work" }];
  assert.deepEqual(H.workOnly(rows).map((x) => x.activity), ["coding", "research"]);
});
