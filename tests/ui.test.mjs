import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const read = (name) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
const moduleUrl = (source) => `data:text/javascript;base64,${Buffer.from(ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText).toString("base64")}`;
const uiUrl = moduleUrl(read("ui.ts"));
const { NAV_ITEMS, navigationMarkup, preferenceSwitch } = await import(uiUrl);
const settingsUrl = moduleUrl(read("settings-view.ts")
  .replace('"./ui"', JSON.stringify(uiUrl))
  .replace('"./theme"', JSON.stringify(moduleUrl(read("theme.ts")))));
const { preferencesMarkup } = await import(settingsUrl);
const css = read("desktop-theme.css");
const main = read("main.ts");

test("all eleven navigation items retain unique routes, labels and code-native icons", () => {
  assert.deepEqual(NAV_ITEMS.map(({ id }) => id), ["overview", "profiles", "subscriptions", "proxies", "programs", "routing", "rules", "connections", "logs", "diagnostics", "settings"]);
  assert.equal(new Set(NAV_ITEMS.map(({ id }) => id)).size, 11);
  assert.equal((navigationMarkup.match(/<svg /g) ?? []).length, 11);
  assert.equal((navigationMarkup.match(/aria-current="page"/g) ?? []).length, 1);
  assert.equal((navigationMarkup.match(/aria-hidden="true"/g) ?? []).length, 11);
});
test("switches retain a native checked input and keyboard-focusable control", () => {
  assert.match(preferenceSwitch("settings-launch"), /id="settings-launch" type="checkbox" role="switch"/);
  assert.match(css, /input:focus-visible \+ \.toggle-track/);
  assert.match(css, /prefers-reduced-motion/);
  assert.match(css, /forced-colors/);
});
test("preference groups preserve all backend binding IDs without duplication", () => {
  const ids = [...preferencesMarkup.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]);
  assert.equal(new Set(ids).size, ids.length);
  for (const id of ["settings-mode", "settings-mixed-port", "settings-controller-port", "settings-launch", "settings-global-traffic", "settings-retention", "settings-form", "update-preferences-form", "app-update-current", "app-update-check", "app-update-save", "app-update-download", "app-update-cancel", "app-update-install", "app-update-message", "network-mode-help"]) assert.ok(ids.includes(id), id);
  assert.match(preferencesMarkup, /安装和重启会短暂中断代理连接/);
});
test("network modes remain native radio drafts, not immediate proxy mutations", () => {
  for (const mode of ["manual", "system_proxy", "tun"]) assert.match(preferencesMarkup, new RegExp(`name="settings-network-mode" value="${mode}"`));
  const handler = main.slice(main.indexOf('radio.addEventListener("change"'), main.indexOf('$("#settings-form")!.addEventListener("submit"'));
  assert.match(handler, /\.value = radio\.value/);
  assert.doesNotMatch(handler, /api\.|switchNetworkMode|startRuntime|stopRuntime/);
});
test("shared theme tokens own the appearance and nav has one accessible active state", () => {
  assert.match(css, /--bg: #e8edfa/);
  assert.match(css, /--primary-background: #2764eb/);
  assert.match(css, /backdrop-filter: blur\(var\(--glass-blur\)\)/);
  assert.match(css, /prefers-reduced-transparency/);
  assert.doesNotMatch(navigationMarkup, /nav-icon-(blue|purple|green|orange|yellow|red)/);
  assert.match(css, /\.sidebar \.nav-item\[aria-current="page"\]/);
  assert.doesNotMatch(read("styles.css"), /body:has\(#\w+-view/);
  assert.doesNotMatch(main, /<h1[^>]*>应用状态/);
});
test("idle update hides status details, never the check/preferences controls", () => {
  assert.ok(preferencesMarkup.indexOf('id="app-update-check"') < preferencesMarkup.indexOf('id="app-update-feedback"'));
  assert.ok(preferencesMarkup.indexOf('id="app-update-save"') < preferencesMarkup.indexOf('id="app-update-feedback"'));
  assert.match(main, /#app-update-feedback[\s\S]*?appUpdateStatus\.phase === "idle"/);
});
