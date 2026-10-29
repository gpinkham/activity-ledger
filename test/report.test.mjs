// Tests for the weekly report's week choices, file name and hide setting
// (src/lib/report.ts), and the tray item. Run with `npm run test:ui`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { buildSync } from "esbuild";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const cache = fileURLToPath(new URL("../node_modules/.cache/", import.meta.url));
mkdirSync(cache, { recursive: true });
const out = mkdtempSync(join(cache, "ci-ui-report-"));
const build = (entry, name) => {
  const outfile = join(out, name);
  buildSync({
    entryPoints: [fileURLToPath(new URL(entry, import.meta.url))],
    bundle: true, format: "esm", platform: "node", jsx: "automatic", outfile,
    external: ["react", "react-dom", "react/jsx-runtime"], logLevel: "silent",
  });
  return outfile;
};
const R = await import(build("../src/lib/report.ts", "report.mjs"));
const { TrayPanel } = await import(build("../src/screens/TrayPanel.tsx", "TrayPanel-report.mjs"));
rmSync(out, { recursive: true, force: true });

test("the default file name is the ISO week of the Monday", () => {
  assert.equal(R.reportFileName("2026-10-26"), "activity-ledger-2026-W44.md");
  assert.equal(R.reportFileName("2026-01-05"), "activity-ledger-2026-W02.md");
  // 29 Dec 2025 is in ISO week 1 of 2026.
  assert.equal(R.reportFileName("2025-12-29"), "activity-ledger-2026-W01.md");
  // 28 Dec 2026 is in week 53 of 2026.
  assert.equal(R.reportFileName("2026-12-28"), "activity-ledger-2026-W53.md");
});

test("last week and this week start on local Mondays", () => {
  const wed = new Date(2026, 9, 28, 15, 0); // Wed 28 Oct 2026, local
  assert.equal(R.weekStart("this", wed), "2026-10-26");
  assert.equal(R.weekStart("last", wed), "2026-10-19");
  const sunLate = new Date(2026, 10, 1, 23, 30); // Sun 1 Nov 2026, 23:30 local
  assert.equal(R.weekStart("this", sunLate), "2026-10-26");
  const mon = new Date(2026, 10, 2, 0, 5); // Mon 2 Nov, just after midnight
  assert.equal(R.weekStart("this", mon), "2026-11-02");
  assert.equal(R.weekStart("last", mon), "2026-10-26");
  assert.equal(R.REPORT_WEEK_LABELS.last, "Last week");
  assert.equal(R.REPORT_WEEK_LABELS.this, "This week so far");
});

test("the hide setting round-trips and is off for an older settings file", () => {
  const base = { ollama_url: "loopback", ollama_model: "m", local_context_limit: 1,
    local_output_limit: 1, model_labels_enabled: false, allow_remote_ollama: false,
    monthly_budget_usd: 300, amber_pct: 80, red_pct: 100, show_in_menu_bar: true, report_hide_projects: false };
  assert.equal(R.hideProjectsOf(base), false);
  const on = R.withHideProjects(base, true);
  assert.equal(R.hideProjectsOf(on), true);
  assert.deepEqual({ ...on, report_hide_projects: false }, base, "nothing else changes");
  assert.equal(R.hideProjectsOf(JSON.parse(JSON.stringify(on))), true, "survives a save and load");
  assert.equal(R.hideProjectsOf(R.withHideProjects(on, false)), false);
  const { report_hide_projects, ...older } = base;
  assert.equal(R.hideProjectsOf(older), false);
  assert.equal(R.hideProjectsOf(null), false);
});

const view = { day: "2026-10-15", rate_source: "list", budget: {
  monthly_budget: 0, days_in_month: 31, days_elapsed: 15, days_left: 17, daily_allowance: 0, today: 1,
  today_share: 0, month_to_date: 10, on_pace: 0, projected: 20, remaining_per_day: 0, ring: "none" } };

test("the tray offers Save weekly report, which only asks the main window", () => {
  let asked = 0;
  const html = renderToStaticMarkup(createElement(TrayPanel, { view, split: null, error: null, onOpen: () => {}, onSaveReport: () => { asked++; } }));
  assert.match(html, /data-save-report[^>]*>Save weekly report</);
  assert.equal(asked, 0, "rendering does nothing");
  const without = renderToStaticMarkup(createElement(TrayPanel, { view, split: null, error: null, onOpen: () => {} }));
  assert.doesNotMatch(without, /Save weekly report/);
});

test("the new UI strings have no en or em dash", () => {
  for (const f of ["../src/lib/report.ts", "../src/components/WeeklyReportButton.tsx", "../src/screens/TrayPanel.tsx"]) {
    const text = readFileSync(fileURLToPath(new URL(f, import.meta.url)), "utf8");
    assert.doesNotMatch(text, /[–—]/, f);
  }
});
