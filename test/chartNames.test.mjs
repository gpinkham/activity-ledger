// Every chart series with a fixed data key sets a display name, so a tooltip or
// legend never shows a raw key such as "cost_usd". Run with `npm run test:ui`.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";

const dir = new URL("../src/screens/", import.meta.url);

/** Each <Area>, <Bar> or <Line> tag whose dataKey is a string literal. */
function seriesTags(source) {
  const out = [];
  for (const m of source.matchAll(/<(Area|Bar|Line)\s/g)) {
    const rest = source.slice(m.index);
    const ends = [rest.indexOf("/>"), rest.indexOf(">\n")].filter((i) => i >= 0);
    const tag = rest.slice(0, Math.min(...ends) + 2);
    if (/dataKey="[^"]+"/.test(tag)) out.push(tag);
  }
  return out;
}

test("every fixed-key chart series has a readable name", () => {
  const missing = [];
  let seen = 0;
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".tsx"))) {
    for (const tag of seriesTags(readFileSync(new URL(file, dir), "utf8"))) {
      seen += 1;
      if (!/\sname="[^"_]+"/.test(tag)) missing.push(`${file}: ${tag.split("\n")[0].trim()}`);
    }
  }
  assert.ok(seen >= 10, `the scan finds the charts (${seen})`);
  assert.deepEqual(missing, []);
});

test("the cost over time tooltip says what the number is", () => {
  const overview = readFileSync(new URL("Overview.tsx", dir), "utf8");
  const [area] = seriesTags(overview);
  assert.match(area, /dataKey="cost_usd"/);
  assert.match(area, /name="Est\. cost"/);
});
