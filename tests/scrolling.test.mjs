import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
const source = readFileSync(new URL("../src/scrolling.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText;
const { coarseWheelDelta, nextScrollTarget, scrollFrame } =
  await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);

test("coarse wheel conversion keeps precision input native", () => {
  for (const delta of [0, 0.2, -12, 39, 80.5, NaN, Infinity]) {
    assert.equal(coarseWheelDelta(delta, 0, 600), null);
  }
  assert.equal(coarseWheelDelta(120, 0, 600), 120);
  assert.equal(coarseWheelDelta(-3, 1, 600), -96);
  assert.equal(coarseWheelDelta(1, 2, 600), 600);
  assert.equal(coarseWheelDelta(120, 9, 600), null);
});

test("continuous wheel accumulates, reverses immediately and clamps at boundaries", () => {
  assert.equal(nextScrollTarget(20, 100, 120, 600), 220);
  assert.equal(nextScrollTarget(20, 100, -120, 600), 0);
  assert.equal(nextScrollTarget(500, 580, 120, 600), 600);
  assert.equal(nextScrollTarget(20, null, -120, 600), 0);
});

test("animation converges with integer and fractional scroll positions without overshoot", () => {
  for (const [start, target] of [[0, 100], [100, 0], [0, 0.3], [600, 425.8]]) {
    let current = start;
    for (let i = 0; i < 200 && Math.abs(current - target) > 0.5; i++) {
      const next = scrollFrame(current, target, 16);
      assert.ok(next >= Math.min(current, target) && next <= Math.max(current, target));
      current = Number.isInteger(target) ? Math.round(next) : next;
    }
    assert.ok(Math.abs(current - target) <= 0.5);
  }
  assert.equal(scrollFrame(0, 1, 1), 1);
  assert.equal(scrollFrame(100, 100, 16), 100);
});
