// Tests for scripts/sign-notarize.sh, run against stand-in tools on PATH: no
// keychain, Apple service or build is touched. Run with `npm run test:ui`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { spawnSync } from "node:child_process";

const identity = "Developer ID Application: Test Name (TEAM1)";

/** A copy of the script in a fresh tree, with stand-ins for every outside tool. */
function setup({ targets, stapler = true }) {
  const root = mkdtempSync(join(tmpdir(), "al-sign-"));
  mkdirSync(join(root, "scripts"));
  copyFileSync(new URL("../scripts/sign-notarize.sh", import.meta.url), join(root, "scripts", "sign-notarize.sh"));
  mkdirSync(join(root, "src-tauri", "target", "universal-apple-darwin", "release", "bundle", "macos", "Test.app"), { recursive: true });
  writeFileSync(join(root, "src-tauri", "tauri.conf.json"), JSON.stringify({ version: "9.9.9" }));
  const bin = join(root, "bin");
  mkdirSync(bin);
  const keychain = join(root, "keychain-asked");
  const stub = (name, body) => {
    writeFileSync(join(bin, name), `#!/bin/sh\n${body}\n`);
    chmodSync(join(bin, name), 0o755);
  };
  stub("uname", "echo Darwin");
  stub("security", `echo asked >> "${keychain}"; echo '  1) ABC "${identity}"'`);
  stub("rustup", targets.map((t) => `echo ${t}`).join("\n"));
  stub("npm", "echo npm should not run >&2; exit 3");
  // Many lines after the match, so a `codesign | grep -q` pipe would break.
  stub("codesign", `[ "$1" = "-d" ] && { echo "flags=0x10000(runtime)" >&2; echo "Authority=${identity}" >&2; i=0; while [ $i -lt 20000 ]; do echo "Detail line $i" >&2; i=$((i+1)); done; }; exit 0`);
  stub("ditto", 'for a; do last="$a"; done; : > "$last"');
  stub("xcrun", [
    'case "$1 $2" in',
    // As on older macOS: `stapler help` fails even where the tool is there.
    '  "stapler help") exit 1 ;;',
    `  "--find stapler") ${stapler ? "echo /usr/bin/stapler" : "exit 72"} ;;`,
    '  "notarytool submit") echo "  id: 1234-abcd"; echo "  status: Invalid"; exit 69 ;;',
    '  "notarytool log") echo "NOTARY LOG $3" ;;',
    "esac",
    "exit 0",
  ].join("\n"));
  const run = (...args) => spawnSync("bash", [join(root, "scripts", "sign-notarize.sh"), ...args], {
    encoding: "utf8",
    env: { PATH: `${bin}:${dirname(process.execPath)}:/usr/bin:/bin`, AL_SIGNING_IDENTITY: identity, AL_OUT_DIR: join(root, "out"), HOME: root },
  });
  return { root, run, keychainAsked: () => existsSync(keychain) };
}

test("a missing Rust target stops the universal build with the command to add it", () => {
  const s = setup({ targets: ["x86_64-apple-darwin"] });
  const r = s.run();
  assert.equal(r.status, 1, r.stderr);
  assert.ok(r.stderr.includes("run: rustup target add aarch64-apple-darwin"), r.stderr);
  assert.ok(!s.keychainAsked(), "stopped before the keychain lookup");
});

test("a failed submit still fetches the notary log", () => {
  const s = setup({ targets: [] });
  const r = s.run("--skip-build");
  assert.equal(r.status, 1, r.stdout + r.stderr);
  assert.ok(r.stdout.includes("NOTARY LOG 1234-abcd"), r.stdout + r.stderr);
  assert.ok(r.stderr.includes("notarization was not accepted"), r.stderr);
  assert.ok(s.keychainAsked(), "the identity check ran, against the stand-in");
});

test("a missing stapler stops it before the keychain; a failing `stapler help` does not", () => {
  const missing = setup({ targets: [], stapler: false });
  const r = missing.run("--skip-build");
  assert.equal(r.status, 1, r.stdout + r.stderr);
  assert.ok(r.stderr.includes("xcrun stapler is missing"), r.stderr);
  assert.ok(!missing.keychainAsked());
  const present = setup({ targets: [] });
  const ok = present.run("--skip-build");
  assert.ok(!ok.stderr.includes("stapler is missing"), ok.stderr);
  assert.ok(present.keychainAsked(), "it went on past the tool checks");
});

test("uncommitted changes stop it before the keychain: the zip is named after the commit", () => {
  const s = setup({ targets: [] });
  const git = (...a) => spawnSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.invalid", ...a], { cwd: s.root, encoding: "utf8" });
  git("init", "-q");
  git("add", "src-tauri/tauri.conf.json");
  git("commit", "-q", "-m", "t");
  writeFileSync(join(s.root, "src-tauri", "tauri.conf.json"), JSON.stringify({ version: "9.9.10" }));
  const r = s.run("--skip-build");
  assert.equal(r.status, 1, r.stdout + r.stderr);
  assert.ok(r.stderr.includes("uncommitted changes"), r.stderr);
  assert.ok(!s.keychainAsked());
});
