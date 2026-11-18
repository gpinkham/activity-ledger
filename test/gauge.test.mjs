// Tests for the menu bar gauge's wording (src/lib/gauge.ts) and its panel
// (src/screens/TrayPanel.tsx), rendered to HTML in each ring state. Run with
// `npm run test:ui`. The TypeScript is compiled with the esbuild Vite already uses.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { buildSync } from "esbuild";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

// Inside node_modules so the bundles find the same React the test renders with.
const cache = fileURLToPath(new URL("../node_modules/.cache/", import.meta.url));
mkdirSync(cache, { recursive: true });
const out = mkdtempSync(join(cache, "ci-ui-gauge-"));
const build = (entry, name) => {
  const outfile = join(out, name);
  buildSync({
    entryPoints: [fileURLToPath(new URL(entry, import.meta.url))],
    bundle: true, format: "esm", platform: "node", jsx: "automatic", outfile,
    external: ["react", "react-dom", "react/jsx-runtime"], logLevel: "silent",
  });
  return pathToFileURL(outfile).href;
};
const G = await import(build("../src/lib/gauge.ts", "gauge.mjs"));
const { TrayPanel } = await import(build("../src/screens/TrayPanel.tsx", "TrayPanel.mjs"));
rmSync(out, { recursive: true, force: true });

const budget = (over = {}) => ({
  monthly_budget: 300, days_in_month: 31, days_elapsed: 15, days_left: 17,
  daily_allowance: 300 / 31, today: 4.1, today_share: 4.1 / (300 / 31), month_to_date: 150,
  on_pace: (300 / 31) * 15, projected: 310, remaining_per_day: 150 / 17, ring: "green", ...over,
});
const view = (b, rate_source = "list") => ({ day: "2026-10-15", budget: b, rate_source });
const html = (props) => renderToStaticMarkup(createElement(TrayPanel, { onOpen: () => {}, error: null, ...props }));

test("the note says what the figures are, by where the rates came from", () => {
  assert.equal(G.rateNote("list"), "Estimate at list price, this Mac only, Claude Code only.");
  assert.equal(G.rateNote("configured"), "Estimate at configured rates, this Mac only, Claude Code only.");
});

test("pace, remaining and split wording", () => {
  assert.equal(G.paceLine(budget({ month_to_date: 150, on_pace: 145.16 })), "$4.84 ahead of an even pace");
  assert.equal(G.paceLine(budget({ month_to_date: 140, on_pace: 145.16 })), "$5.16 under an even pace");
  assert.equal(G.paceLine(budget({ month_to_date: 145.16, on_pace: 145.16 })), "On an even pace");
  assert.equal(G.remainingLine(budget({ remaining_per_day: 8.82, days_left: 17 })), "$8.82 for each of the 17 days left");
  assert.equal(G.remainingLine(budget({ remaining_per_day: 20, days_left: 1 })), "$20.00 for today");
  assert.deepEqual(G.splitShares(3, 1), { work: 75, overhead: 25 });
  assert.equal(G.splitShares(0, 0), null);
});

test("weekdays-only wording", () => {
  const wk = (over = {}) => budget({ weekdays_only: true, days_in_month: 21, days_left: 11, remaining_per_day: 100, ...over });
  assert.equal(G.remainingLine(wk()), "$100.00 for each of the 11 weekdays left");
  assert.equal(G.remainingLine(wk({ days_left: 1, remaining_per_day: 500 })), "$500.00 for today");
  assert.equal(G.remainingLine(wk({ days_left: 1, remaining_per_day: 500, ring: "weekend" })), "$500.00 for the last weekday");
  assert.equal(G.remainingLine(wk({ days_left: 0, remaining_per_day: 0 })), "No weekdays left this month");
  assert.equal(G.remainingLine(budget({ days_left: 17, remaining_per_day: 8.82 })), "$8.82 for each of the 17 days left", "every day: unchanged");
  assert.equal(G.projectedLine(wk({ no_weekday_yet: true, projected: 45 })), "No weekday yet");
  assert.equal(G.projectedLine(wk({ projected: 1909.09 })), "$1909.09");
  assert.equal(G.weekdaysNote(wk()), "Spread over the 21 weekdays this month, Monday to Friday. Weekend spend counts in the month.");
  assert.equal(G.weekdaysNote(budget()), null);
  assert.equal(G.ringWord("weekend"), "Weekend: counted in the month");
  assert.equal(G.ringClass("weekend"), "text-muted");
});

test("a weekend on a weekdays-only budget shows the dollars, grey, with no allowance", () => {
  const b = budget({ weekdays_only: true, ring: "weekend", today: 12.5, today_share: 0, days_in_month: 21, days_left: 6 });
  const h = html({ view: view(b), split: null });
  assert.match(h, /\$12\.50/, "the weekend spend shows");
  assert.ok(!/\$12\.50<span[^>]*> of /.test(h), "with no allowance after it");
  assert.match(h, /data-ring="weekend"[^>]*>Weekend: counted in the month</, "grey words, and no percentage");
  assert.match(h, /class="text-xs text-muted" data-ring="weekend"/);
  assert.match(h, /data-weekdays[^>]*>Spread over the 21 weekdays/);
  assert.match(h, /for each of the 6 weekdays left/);
  assert.ok(!html({ view: view(budget()), split: null }).includes("data-weekdays"), "every day: no note");
});

test("a weekdays-only month that starts on a weekend shows no weekday yet", () => {
  const b = budget({ weekdays_only: true, no_weekday_yet: true, ring: "weekend", days_elapsed: 0, projected: 45, monthly_budget: 30 });
  const h = html({ view: view(b), split: null });
  assert.match(h, /Projected month end.*No weekday yet/);
  assert.ok(!/text-red-500[^"]*"[^>]*>No weekday yet/.test(h), "never red, though 45 is over a 30 budget");
});

test("the weekdays-only setting is off unless an account turns it on", async () => {
  const { readFileSync } = await import("node:fs");
  const card = readFileSync(new URL("../src/components/AccountsCard.tsx", import.meta.url), "utf8");
  assert.match(card, /checked=\{draft\.weekdays_only \?\? false\}/);
  assert.match(card, /Weekdays only/);
});

test("jump wording", () => {
  const j = { kind: "today", times: 2.1, spent: 84, usual: 40, project: null };
  assert.equal(G.jumpLine(j), "Today is 2.1 times a usual day ($84.00 so far, usually about $40.00).");
  assert.equal(G.jumpLine({ ...j, times: 2.0 }), "Today is 2.0 times a usual day ($84.00 so far, usually about $40.00).");
  assert.equal(G.jumpLine({ kind: "recent", times: 1.8, spent: 72, usual: 40, project: null }),
    "Up for 3 days: about 1.8 times the week before ($72.00 a day, usually about $40.00).");
  assert.equal(G.jumpLine({ ...j, project: "new-thing" }),
    "Today is 2.1 times a usual day ($84.00 so far, usually about $40.00). Mostly from new-thing.");
  for (const line of [G.jumpLine(j), G.jumpLine({ ...j, kind: "recent" })]) {
    assert.ok(!/signal|alert|[\u2013\u2014]/i.test(line), line);
  }
});

test("the panel shows the project name the backend gives it, as it is", () => {
  for (const project of ["dev-app", "the home folder"]) {
    const h = html({ view: { ...view(budget()), jump: { kind: "today", times: 2, spent: 80, usual: 40, project } }, split: null });
    assert.ok(h.includes(`Mostly from ${project}.`), project);
  }
});

test("the jump shows as its own line and never changes the ring", () => {
  const jump = { kind: "today", times: 2.1, spent: 84, usual: 40, project: null };
  for (const ring of ["green", "amber", "red"]) {
    const b = budget({ ring });
    const plain = html({ view: view(b), split: null });
    const withJump = html({ view: { ...view(b), jump }, split: null });
    assert.ok(!plain.includes("data-jump"), "no jump, no line");
    assert.match(withJump, /data-jump="today"[^>]*>.*Jump: .*Today is 2\.1 times a usual day/);
    const ringPart = (h) => h.match(/data-ring="[a-z]+"[^>]*>[^<]*/)[0];
    assert.equal(ringPart(withJump), ringPart(plain), `the ring is the same with a jump (${ring})`);
    assert.ok(!/data-jump[^>]*class="[^"]*(red|amber|emerald)/.test(withJump), "and the line takes none of its colours");
  }
  const none = html({ view: { ...view(budget({ ring: "none", monthly_budget: 0 })), jump }, split: null });
  assert.match(none, /data-jump/, "no budget set: the jump still shows");
  const weekend = budget({ weekdays_only: true, ring: "weekend", today_share: 0 });
  const grey = html({ view: { ...view(weekend), jump }, split: null });
  assert.match(grey, /data-ring="weekend"[^>]*>Weekend: counted in the month</, "a weekend keeps its grey ring with a jump");
  assert.match(grey, /data-jump="today"/, "and the jump shows on its own line");
  assert.ok(!html({ view: { ...view(budget()), jump: null }, split: null }).includes("data-jump"));
});

test("the budget form refuses what the gauge cannot use", () => {
  assert.equal(G.checkBudget(300, 80, 100), null);
  assert.equal(G.checkBudget(0, 80, 100), null, "0 is no budget");
  assert.match(G.checkBudget(-1, 80, 100), /dollars/);
  assert.match(G.checkBudget(NaN, 80, 100), /dollars/);
  assert.match(G.checkBudget(300, 100, 100), /below red/);
  assert.match(G.checkBudget(300, 0, 100), /above 0/);
});

test("each ring state renders its colour, its words and the estimate note", () => {
  const cases = [
    ["green", 0.42, "text-emerald-500", "Within today&#x27;s allowance (42%)"],
    ["amber", 0.85, "text-amber-500", "Near today&#x27;s allowance (85%)"],
    ["red", 1.3, "text-red-500", "Over today&#x27;s allowance (130%)"],
  ];
  for (const [ring, share, cls, words] of cases) {
    const h = html({ view: view(budget({ ring, today_share: share })), split: { work_usd: 3, overhead_usd: 1 } });
    assert.match(h, new RegExp(`data-state="${ring}"`));
    assert.match(h, new RegExp(`class="text-xs ${cls}" data-ring="${ring}"`), ring);
    assert.ok(h.includes(words), `${ring}: ${words}`);
    assert.ok(h.includes("$4.10") && h.includes("of $9.68"), "today of the allowance");
    assert.ok(h.includes("$150.00 of $300.00"), "month to date of the budget");
    assert.ok(h.includes("75% / 25%"), "work and overhead");
    assert.ok(h.includes("Estimate at list price, this Mac only, Claude Code only."), "the estimate note");
    assert.ok(h.includes("Open Activity Ledger"));
  }
});

test("over budget the projection shows red and nothing is left per day", () => {
  const h = html({ view: view(budget({ ring: "red", projected: 420, month_to_date: 305, remaining_per_day: 0 })), split: null });
  assert.match(h, /tabular-nums text-red-500">\$420\.00/);
  assert.ok(h.includes("Nothing left this month"), "not $0.00 for each of the days left");
  assert.ok(!h.includes("$0.00 for each"));
  assert.ok(h.includes("working it out"), "the split is still loading");
});

// The worked examples in the spec (HAG-208): $300 in December 2026, an even $9.68.
const december = (over = {}) => budget({ days_in_month: 31, days_elapsed: 8, days_left: 24, daily_allowance: 300 / 31, ...over });

test("case A: the ring is measured against what is left, with a grey note", () => {
  const b = december({ today: 7, today_allowance: 7.5, today_share: 7 / 7.5, month_to_date: 127, remaining_per_day: 173 / 24, ring: "amber" });
  const h = html({ view: view(b), split: null });
  assert.ok(h.includes("$7.00") && h.includes("of $7.50"), "today of today's allowance");
  assert.ok(!h.includes("of $9.68"), "not of the even spread");
  assert.ok(h.includes("Near today&#x27;s allowance (93%)"));
  assert.match(h, /data-allowance-note[^>]*>Even spread \$9\.68, lowered by the month so far</);
  assert.ok(h.includes("$7.21 for each of the 24 days left"), "left per day is unchanged");
  assert.equal(G.allowanceNote(december({ today_allowance: 280 / 21 })), "Even spread $9.68, raised by the month so far");
});

test("the grey note shows only when the two allowances differ by 5% or more", () => {
  const even = 300 / 31;
  assert.equal(G.allowanceNote(december({ today_allowance: even })), null, "the same: one number");
  assert.equal(G.allowanceNote(december({ today_allowance: even * 0.96 })), null, "4% under");
  assert.equal(G.allowanceNote(december({ today_allowance: even * 1.049 })), null, "4.9% over");
  assert.match(G.allowanceNote(december({ today_allowance: even * 0.95 })), /lowered/, "5% under");
  assert.match(G.allowanceNote(december({ today_allowance: even * 1.05 })), /raised/, "5% over");
  assert.equal(G.allowanceNote(december({ today_allowance: 5, ring: "weekend" })), null, "no note on a weekend");
  assert.equal(G.allowanceNote(december({ today_allowance: 0, ring: "none", daily_allowance: 0 })), null, "nor with no budget");
  assert.equal(G.allowanceNote(budget()), null, "an older backend with no today_allowance: no note");
  assert.ok(!html({ view: view(december({ today_allowance: even })), split: null }).includes("data-allowance-note"));
  assert.ok(html({ view: view(budget()), split: null }).includes("of $9.68"), "an older backend: the even spread");
});

test("case B: a budget used up before today is full red with no percentage", () => {
  const b = december({ days_elapsed: 20, days_left: 12, today: 5, today_share: 0, today_allowance: 300 / 31, used_up: true,
    month_to_date: 315, remaining_per_day: 0, projected: 488.25, ring: "red" });
  const h = html({ view: view(b), split: null });
  assert.match(h, /data-ring="red" data-used-up=""[^>]*>Month budget used: \$315\.00 of \$300\.00</);
  assert.ok(!/data-ring="red"[^>]*>[^<]*%/.test(h), "no percentage");
  assert.ok(!h.includes(" of $9.68"), "and no allowance after today's spend");
  assert.ok(!h.includes("data-allowance-note"));
  assert.ok(h.includes("Nothing left this month"));
  assert.match(h, /tabular-nums text-red-500">\$488\.25/, "the projection is red as before");
  assert.equal(G.ringLine(b), "Month budget used: $315.00 of $300.00");
});

test("weekdays only: the weekend and no-weekday-yet renders are unchanged, and nothing left reads the same", () => {
  const wk = (over = {}) => budget({ weekdays_only: true, days_in_month: 23, days_left: 14, remaining_per_day: 12.14, ...over });
  const weekend = wk({ ring: "weekend", today: 30, today_share: 0, today_allowance: 300 / 23, daily_allowance: 300 / 23 });
  const h = html({ view: view(weekend), split: null });
  assert.match(h, /data-ring="weekend"[^>]*>Weekend: counted in the month</);
  assert.ok(!h.includes("data-allowance-note") && !/\$30\.00<span[^>]*> of /.test(h));
  const first = wk({ ring: "weekend", no_weekday_yet: true, today_share: 0, days_left: 21, today_allowance: 100, daily_allowance: 100 });
  assert.match(html({ view: view(first), split: null }), /No weekday yet/);
  assert.equal(G.remainingLine(wk({ month_to_date: 320, monthly_budget: 300, remaining_per_day: 0 })), "Nothing left this month");
  assert.equal(G.remainingLine(budget({ month_to_date: 299.999, monthly_budget: 300, remaining_per_day: 0 })), "Nothing left this month", "a tenth of a cent is nothing left");
  assert.equal(G.remainingLine(budget({ month_to_date: 299.99, monthly_budget: 300, remaining_per_day: 0.01 / 17 })), "$0.00 for each of the 17 days left", "a whole cent is not");
  assert.equal(G.remainingLine(wk({ days_left: 0, month_to_date: 320, monthly_budget: 300, remaining_per_day: 0 })), "No weekdays left this month", "kept");
});

test("with no budget the panel shows the spend, no ring colour, and where to set one", () => {
  const b = budget({ ring: "none", monthly_budget: 0, daily_allowance: 0, today_share: 0, remaining_per_day: 0 });
  const h = html({ view: view(b, "configured"), split: { work_usd: 0, overhead_usd: 0 } });
  assert.match(h, /data-ring="none"/);
  assert.ok(h.includes("No budget set"));
  assert.ok(!h.includes(" of $0.00"), "no allowance to show");
  assert.ok(!h.includes("Pace"), "no pace without a budget");
  assert.ok(h.includes("Budgets are per account"), "every account together has no budget");
  const one = html({ view: { ...view(b), account: "acct:a:o", account_label: "Acme" }, split: null });
  assert.ok(one.includes("Set this account&#x27;s monthly budget"), "one account: where to set its budget");
  assert.ok(one.includes("Acme"), "and which account it is");
  assert.ok(h.includes("no calls yet"));
  assert.ok(h.includes("Estimate at configured rates, this Mac only, Claude Code only."));
});

test("what was typed in the budget card survives a refetch of the settings", () => {
  const saved = { monthly_budget_usd: 300, amber_pct: 80, red_pct: 100 };
  const typed = { ...saved, monthly_budget_usd: 450 };
  assert.equal(G.cardDraft(typed, { ...saved }), typed, "a new fetch does not replace the edits");
  assert.equal(G.cardDraft(null, saved), saved, "nothing typed: the saved settings");
  assert.equal(G.cardDraft(null, undefined), undefined);
});

test("the panel refreshes only while it is shown", () => {
  let loads = 0;
  const live = new Map();
  let next = 1;
  const timers = { set: (f) => { live.set(next, f); return next++; }, clear: (id) => live.delete(id) };
  const tick = () => { for (const f of live.values()) f(); };
  const poll = G.panelPoller(() => loads++, 60_000, timers);
  poll.shown();
  assert.equal(loads, 1, "loads at once when shown");
  tick(); tick();
  assert.equal(loads, 3, "then once a minute");
  poll.hidden();
  assert.equal(poll.running, false);
  tick(); tick(); tick();
  assert.equal(loads, 3, "nothing while hidden");
  poll.shown(); poll.shown();
  assert.equal(live.size, 1, "showing twice keeps one timer");
  assert.equal(loads, 5);
  poll.hidden(); poll.hidden();
  assert.equal(live.size, 0);
});

test("before the first figures arrive, and on an error", () => {
  assert.match(html({ view: null, split: null }), /Reading this month/);
  assert.match(html({ view: null, split: null, error: "no transcripts" }), /no transcripts/);
});

// The menu bar icon has no native menu (tray.rs), so the panel is the only way to
// open the window or quit while the app lives in the menu bar: both buttons are
// there in every state, the loading and error ones included.
test("Open and Quit are in the panel in every state", () => {
  const states = {
    loading: { view: null, split: null },
    error: { view: null, split: null, error: "no transcripts" },
    "no budget": { view: view(budget({ ring: "none", today_share: 0 })), split: null },
    green: { view: view(budget()), split: { work_usd: 3, overhead_usd: 1 } },
  };
  for (const [name, props] of Object.entries(states)) {
    const h = html({ ...props, onQuit: () => {}, onSaveReport: () => {} });
    assert.match(h, />Open Activity Ledger</, name);
    assert.match(h, /data-quit[^>]*>Quit Activity Ledger</, name);
  }
  assert.ok(!html({ view: null, split: null, onSaveReport: () => {} }).includes("data-save-report"),
    "no weekly report before there are figures");
  assert.ok(!html({ view: null, split: null }).includes("data-quit"), "no Quit when no handler is given");
});

test("with two accounts or more the panel offers a switcher, and a pick is sent", () => {
  const accounts = [{ id: "acct:me:o1", label: "Personal", colour: "#3B82F6" }, { id: "acct:me:acme", label: "Acme", colour: "#F59E0B" }];
  const v = { ...view(budget()), account: "acct:me:acme", account_label: "Acme" };
  const h = html({ view: v, split: null, accounts, onPickAccount: () => {} });
  assert.match(h, /data-switcher/);
  assert.match(h, />All accounts</);
  assert.match(h, /<option value="acct:me:acme" selected="">Acme<\/option>/, "the account the ring follows is picked");
  assert.ok(h.includes("#F59E0B"), "in its colour");
  const single = html({ view: v, split: null, accounts: accounts.slice(0, 1), onPickAccount: () => {} });
  assert.ok(!single.includes("data-switcher"), "one account: nothing to switch");
  assert.match(single, /data-account-label[^>]*>Acme</, "but its name still shows");
});

test("no en or em dashes in what the panel or the budget card say", async () => {
  const { readFileSync } = await import("node:fs");
  for (const f of ["../src/screens/TrayPanel.tsx", "../src/lib/gauge.ts", "../src/components/BudgetCard.tsx", "../src/screens/TrayApp.tsx",
    "../src/components/AccountsCard.tsx", "../src/components/Accounts.tsx", "../src/lib/accounts.ts"]) {
    const text = readFileSync(new URL(f, import.meta.url), "utf8");
    assert.ok(!/[\u2013\u2014]/.test(text), f);
  }
});

test("the platform words come from the webview's user agent", () => {
  assert.equal(G.platformOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15"), "macos");
  assert.equal(G.platformOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/120.0"), "windows");
  assert.equal(G.platformOf("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15"), "linux");
  assert.equal(G.platformOf(""), "macos", "unknown: the macOS words, as before");
  for (const p of ["macos", "windows", "linux"]) assert.ok(G.PLATFORM_WORDS[p].bar, p);
});
