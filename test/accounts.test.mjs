// Tests for how the window treats Claude accounts (src/lib/accounts.ts). Run with
// `npm run test:ui`. The TypeScript is compiled with the esbuild Vite already uses.
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { buildSync } from "esbuild";

const cache = fileURLToPath(new URL("../node_modules/.cache/", import.meta.url));
mkdirSync(cache, { recursive: true });
const out = mkdtempSync(join(cache, "ci-ui-accounts-"));
const outfile = join(out, "accounts.mjs");
buildSync({
  entryPoints: [fileURLToPath(new URL("../src/lib/accounts.ts", import.meta.url))],
  bundle: true, format: "esm", platform: "node", outfile, logLevel: "silent",
});
const A = await import(pathToFileURL(outfile).href);
rmSync(out, { recursive: true, force: true });

const settings = (over = {}) => ({ label: null, colour: null, hidden: false, monthly_budget_usd: 0, amber_pct: 80, red_pct: 100, ...over });
const account = (id, over = {}) => ({
  id, label: id, default_label: id, plan: null, email: null, org_name: null, colour: "#3B82F6",
  dirs: [{ path: `/home/me/.${id}`, origin: "Windows", custom: false, own_account: id }], settings: settings(), ...over,
});
const personal = account("personal", { plan: "Max", email: "me@example.com",
  dirs: [{ path: "C:/Users/me/.claude", origin: "Windows", custom: false, own_account: "personal" },
    { path: "//wsl.localhost/Ubuntu/home/me/.claude", origin: "WSL · Ubuntu", custom: false, own_account: "personal" }] });
const work = account("work", { plan: "Enterprise", label: "Acme" });
const old = account("old", { settings: settings({ hidden: true }) });

test("hidden accounts are not offered", () => {
  assert.deepEqual(A.visibleAccounts([personal, work, old]).map((a) => a.id), ["personal", "work"]);
  assert.deepEqual(A.visibleAccounts(undefined), []);
});

test("a picked account that is gone or hidden falls back to all accounts", () => {
  const all = [personal, work, old];
  assert.equal(A.pickedAccount(all, "work"), "work");
  assert.equal(A.pickedAccount(all, "old"), undefined, "hidden since it was picked");
  assert.equal(A.pickedAccount(all, "gone"), undefined);
  assert.equal(A.pickedAccount(undefined, "work"), "work", "kept until the accounts load");
  assert.equal(A.pickedAccount(all, undefined), undefined);
});

test("rows carry account chips only when they can be from two accounts", () => {
  assert.equal(A.showAccountChips([personal, work], undefined), true);
  assert.equal(A.showAccountChips([personal, work], "work"), false, "one account picked");
  assert.equal(A.showAccountChips([personal, old], undefined), false, "the other is hidden");
  assert.equal(A.showAccountChips([personal], undefined), false);
});

test("an account says its plan, login and where its folders are", () => {
  assert.equal(A.accountDetail(personal), "Max · me@example.com");
  assert.equal(A.accountDetail(old), "");
  assert.equal(A.accountOrigins(personal), "Windows + WSL · Ubuntu");
  assert.equal(A.accountById([personal, work], "work").label, "Acme");
  assert.equal(A.accountById([personal, work], null), undefined);
});

test("a colour is # and six hex digits", () => {
  assert.equal(A.checkColour("#3B82F6"), null);
  assert.equal(A.checkColour("#3b82f6"), null);
  assert.match(A.checkColour("blue"), /six hex digits/);
  assert.match(A.checkColour("#FFF"), /six hex digits/);
});
