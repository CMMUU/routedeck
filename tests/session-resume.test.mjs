import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const read = name => readFileSync(new URL(`../${name}`, import.meta.url), "utf8");
const source = ts.transpileModule(read("src/session-resume.ts"), {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext },
}).outputText;
const { sessionResumeHelp, sessionResumePresentation } = await import(`data:text/javascript;base64,${Buffer.from(source).toString("base64")}`);

test("missing restore status never claims success or starts a core", () => {
  const view = sessionResumePresentation(null);
  assert.equal(view.visible, false);
  assert.equal(view.busy, false);
  assert.match(view.message, /尚未读取/);
});

test("pending and restoring show progress, not connection success", () => {
  for (const phase of ["pending", "restoring"]) {
    const view = sessionResumePresentation({ phase, message: "正在检查本地核心。" });
    assert.equal(view.busy, true);
    assert.equal(view.visible, true);
    assert.doesNotMatch(view.title, /成功|已连接/);
  }
});

test("paused restoration remains visible with its actual safety reason", () => {
  const message = "其他代理正在使用系统设置，请手动处理。";
  assert.deepEqual(sessionResumePresentation({ phase: "paused", message }), {
    busy: false, visible: true, title: "上次运行状态暂未恢复", message,
  });
});

test("idle and restored avoid a permanent global success banner", () => {
  for (const phase of ["idle", "restored"]) {
    assert.equal(sessionResumePresentation({ phase, message: "状态已读取。" }).visible, false);
  }
});

test("restore help distinguishes login launch, manual opening and opt out", () => {
  assert.match(sessionResumeHelp(false, true), /手动打开应用才会恢复/);
  assert.match(sessionResumeHelp(true, true), /登录后自动打开应用并恢复/);
  assert.match(sessionResumeHelp(true, true), /上次已停止则保持停止/);
  assert.match(sessionResumeHelp(true, true), /不会自动接入 Codex 或启动其他程序/);
  assert.doesNotMatch(sessionResumeHelp(false, false), /登录后自动/);
  assert.match(sessionResumeHelp(false, false), /不会自动启动代理核心/);
  assert.match(sessionResumeHelp(true, true, true), /托盘后台恢复/);
  assert.doesNotMatch(sessionResumeHelp(true, true, true), /自动打开应用/);
});

test("settings save, polling and event paths are wired to the same persisted option", () => {
  const main = read("src/main.ts");
  assert.match(main, /restoreLastSession:.*#settings-restore-session/);
  assert.match(main, /listen<SessionResumeStatus>\("session-resume-status"/);
  assert.match(main, /revision !== sessionResumeRevision/);
  assert.match(read("src/api.ts"), /invoke<SessionResumeStatus>\("get_session_resume_status"\)/);
  assert.match(read("src/settings-view.ts"), /id="session-resume-status" role="status" aria-live="polite"/);
});

test("late initial status reads cannot overwrite a newer completed-resume refresh", async () => {
  let finishFirst;
  const first = new Promise(resolve => { finishFirst = resolve; });
  let calls = 0;
  const noop = () => {};
  const context = {
    baseReadSequence: 0, runtimeMutationRevision: 0,
    runtimeActionInFlight: false, networkModeSwitching: false, settingsSaving: false,
    sessionResumeReadBusy: true,
    themeController: { mutationRevision: 0, sync: () => true },
    store: {}, action: async (_message, run) => run(),
    api: new Proxy({ settings: () => ++calls === 1 ? first : Promise.resolve({ networkMode: "tun" }) }, {
      get: (target, key) => target[key] ?? (async () => null),
    }),
    renderHeader: noop, renderOverview: noop, renderProfiles: noop, renderSubscriptions: noop,
    renderSettings: noop, renderOpenAiPolicy: noop, renderGlobalTraffic: noop, scheduleAutomaticUpdateCheck: noop,
  };
  const main = read("src/main.ts");
  const code = main.slice(main.indexOf("async function refreshBase()"), main.indexOf("function renderHeader()"));
  vm.createContext(context);
  vm.runInContext(ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2020 } }).outputText, context);
  const old = context.refreshBase();
  assert.equal(await context.refreshBase(), true);
  finishFirst({ networkMode: "manual" });
  assert.equal(await old, false);
  assert.equal(context.store.settings.networkMode, "tun");
});
