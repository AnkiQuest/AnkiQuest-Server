// The WebAssembly module against the fixture cases, the way a page loads it: no imports, plain
// numbers. ANKIQUEST_RULES_WASM names the built module (.github/workflows/rules.yml).
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import {test} from "node:test";

type Exports = {
  abi_version: () => number;
  heatmap_intensity: (count: number, dailyAverage: number) => number;
};
type Heatmap = {intensity_cases: {daily_average: number; counts: number[]; levels: number[]}[]};

// The interface version and exports the module promises (crates/rules-wasm/src/lib.rs).
const ABI_VERSION = 1;
const EXPORTS = ["abi_version", "heatmap_intensity", "memory"];

const file = process.env.ANKIQUEST_RULES_WASM;
assert.ok(file, "set ANKIQUEST_RULES_WASM to the built module");
const module = new WebAssembly.Module(fs.readFileSync(file));
const rules = new WebAssembly.Instance(module, {}).exports as unknown as Exports;

const fixtures = path.resolve(import.meta.dirname, "../../../tests/fixtures");
const heatmap = JSON.parse(fs.readFileSync(path.join(fixtures, "review_heatmap.json"), "utf8")) as Heatmap;

test("the module imports nothing and exports the documented interface", () => {
  assert.deepEqual(WebAssembly.Module.imports(module), []);
  assert.deepEqual(WebAssembly.Module.exports(module).map((entry) => entry.name).sort(), EXPORTS);
  assert.equal(rules.abi_version(), ABI_VERSION);
});

test("every heatmap intensity case matches", () => {
  assert.ok(heatmap.intensity_cases.length > 0);
  for (const {daily_average: average, counts, levels} of heatmap.intensity_cases) {
    assert.deepEqual(counts.map((count) => rules.heatmap_intensity(count, average)), levels, `daily_average ${average}`);
  }
});
