import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const source = readFileSync(new URL("../src/theme.ts", import.meta.url), "utf8");
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText;
const { THEME_OPTIONS, ThemeController, normalizeTheme, resolveTheme, themeColorScheme } =
  await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);

function fixture(persist = async (theme) => theme) {
  const renders = [];
  const calls = [];
  let dark = false;
  const controller = new ThemeController({
    systemDark: () => dark,
    persist: async (theme) => { calls.push(theme); return persist(theme); },
    render: (state) => renders.push(state),
  });
  return { controller, calls, renders, system: (value) => { dark = value; controller.refresh(); } };
}

test("four named choices keep purple separate from ordinary dark", () => {
  assert.deepEqual(THEME_OPTIONS.map(({ id }) => id), ["light", "dark", "purple", "system"]);
  assert.equal(THEME_OPTIONS.find(({ id }) => id === "purple").label, "深紫");
});

test("unknown stored preferences fall back to system", () => {
  for (const value of [null, undefined, "unknown", {}, 42]) assert.equal(normalizeTheme(value), "system");
  for (const { id } of THEME_OPTIONS) assert.equal(normalizeTheme(id), id);
});

test("system follows brightness only and never selects purple", () => {
  assert.equal(resolveTheme("system", false), "light");
  assert.equal(resolveTheme("system", true), "dark");
  for (const theme of ["light", "dark", "purple"]) {
    assert.equal(resolveTheme(theme, false), theme);
    assert.equal(resolveTheme(theme, true), theme);
  }
  assert.equal(themeColorScheme("light"), "light");
  assert.equal(themeColorScheme("dark"), "dark");
  assert.equal(themeColorScheme("purple"), "dark");
});

test("loading a persisted choice never writes settings", () => {
  const f = fixture();
  f.controller.sync("purple");
  assert.equal(f.controller.snapshot.resolved, "purple");
  assert.deepEqual(f.calls, []);
});

test("a selection previews immediately and confirms only after save", async () => {
  let finish;
  const f = fixture(() => new Promise((resolve) => { finish = resolve; }));
  f.controller.sync("light");
  const pending = f.controller.select("purple");
  assert.deepEqual(f.controller.snapshot, { preference: "light", selected: "purple", resolved: "purple", saving: true });
  finish("purple");
  assert.equal(await pending, true);
  assert.deepEqual(f.controller.snapshot, { preference: "purple", selected: "purple", resolved: "purple", saving: false });
});

test("failed save restores the last confirmed appearance", async () => {
  const f = fixture(async () => { throw new Error("disk unavailable"); });
  f.controller.sync("dark");
  await assert.rejects(f.controller.select("light"), /disk unavailable/);
  assert.equal(f.controller.snapshot.resolved, "dark");
  assert.equal(f.controller.snapshot.saving, false);
  assert.equal(f.renders.at(-2).selected, "light");
  assert.equal(f.renders.at(-1).selected, "dark");
});

test("rapid clicks do not create concurrent preference writes", async () => {
  let finish;
  const f = fixture(() => new Promise((resolve) => { finish = resolve; }));
  const pending = f.controller.select("dark");
  assert.equal(await f.controller.select("purple"), false);
  assert.deepEqual(f.calls, ["dark"]);
  finish("dark");
  await pending;
});

test("selecting the current preference is a no-op", async () => {
  const f = fixture();
  f.controller.sync("light");
  assert.equal(await f.controller.select("light"), false);
  assert.deepEqual(f.calls, []);
});

test("invalid persisted response rolls back instead of corrupting UI state", async () => {
  const f = fixture(async () => "invalid");
  f.controller.sync("light");
  await assert.rejects(f.controller.select("dark"), /无效设置/);
  assert.equal(f.controller.snapshot.preference, "light");
});

test("stale refreshes do not overwrite a pending preview", async () => {
  let finish;
  const f = fixture(() => new Promise((resolve) => { finish = resolve; }));
  f.controller.sync("light");
  const pending = f.controller.select("purple");
  f.controller.sync("light");
  assert.equal(f.controller.snapshot.selected, "purple");
  finish("purple");
  await pending;
});

test("an earlier refresh cannot overwrite a theme after save completes", async () => {
  const f = fixture();
  f.controller.sync("light");
  const revisionAtRead = f.controller.mutationRevision;
  await f.controller.select("purple");
  assert.equal(f.controller.sync("light", revisionAtRead), false);
  assert.equal(f.controller.snapshot.preference, "purple");
  assert.equal(f.controller.sync("purple", f.controller.mutationRevision), true);
});

test("a read started during a write is stale after that write finishes", async () => {
  let finish;
  const f = fixture(() => new Promise((resolve) => { finish = resolve; }));
  f.controller.sync("light");
  const pending = f.controller.select("dark");
  const revisionAtRead = f.controller.mutationRevision;
  finish("dark");
  await pending;
  assert.equal(f.controller.sync("light", revisionAtRead), false);
  assert.equal(f.controller.snapshot.preference, "dark");
});

test("live system appearance updates do not write preferences", () => {
  const f = fixture();
  f.controller.sync("system");
  f.system(true);
  assert.equal(f.controller.snapshot.resolved, "dark");
  f.system(false);
  assert.equal(f.controller.snapshot.resolved, "light");
  assert.equal(f.controller.snapshot.preference, "system");
  assert.deepEqual(f.calls, []);
});

test("explicit purple stays purple when system appearance changes", () => {
  const f = fixture();
  f.controller.sync("purple");
  f.system(true);
  f.system(false);
  assert.equal(f.controller.snapshot.resolved, "purple");
  assert.deepEqual(f.calls, []);
});

test("invalid selection cannot reach persistence", async () => {
  const f = fixture();
  await assert.rejects(f.controller.select("invalid"), /无效主题/);
  assert.deepEqual(f.calls, []);
});

test("navigation stays rounded and hidden scrollbars preserve accessible scrolling", () => {
  const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
  const main = readFileSync(new URL("../src/main.ts", import.meta.url), "utf8");

  assert.match(css, /\.nav-list\s*\{[\s\S]*?background:\s*transparent;[\s\S]*?box-shadow:\s*none;\s*\}/);
  assert.match(css, /\.nav-item\s*\{[\s\S]*?overflow:\s*hidden;[\s\S]*?border-radius:\s*10px;/);
  const desktop = readFileSync(new URL("../src/desktop-theme.css", import.meta.url), "utf8");
  assert.match(desktop, /scrollbar-width:\s*none;\s*scrollbar-gutter:\s*auto/);
  assert.match(desktop, /\*::-webkit-scrollbar\s*\{\s*display:\s*none;\s*width:\s*0;\s*height:\s*0/);
  assert.doesNotMatch(css + desktop, /scrollbar-width:\s*thin|scrollbar-gutter:\s*stable/);
  assert.match(desktop, /scroll-behavior:\s*smooth/);
  assert.match(desktop, /prefers-reduced-motion:\s*reduce[\s\S]*scroll-behavior:\s*auto/);
  assert.match(main, /id="page-scroll" tabindex="0" role="region"/);
  assert.doesNotMatch(main, /rgba\(81,\s*45,\s*120|#c19aff/i);
});
