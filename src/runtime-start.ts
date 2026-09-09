import type { AppSettings, NetworkMode, RuntimeStatus } from "./types";

export type RuntimeStartMode = "system_proxy" | "tun" | "previous";
type StartResult =
  | { kind: "started" | "skipped" | "needs-profile" | "cancelled" }
  | { kind: "failed"; error: unknown; restored: boolean; rollbackError?: unknown };

export function canStartRuntime(runtime: RuntimeStatus | null): boolean {
  return Boolean(runtime && ["uninitialized", "stopped", "crashed"].includes(runtime.phase));
}

type StartContext = {
  state(): { settings: AppSettings | null; runtime: RuntimeStatus | null; systemProxyActive: boolean; hasProfile: boolean; busy: boolean };
  setBusy(busy: boolean): void;
  setSettings(settings: AppSettings): void;
  setRuntime(runtime: RuntimeStatus): void;
  readRuntime(): Promise<RuntimeStatus>;
  readSettings(): Promise<AppSettings>;
  setNetworkMode(mode: NetworkMode): Promise<AppSettings>;
  startActive(): Promise<RuntimeStatus>;
  ensureTunReady(): Promise<boolean>;
  refresh(): Promise<void>;
};

/** The toolbar's explicit start intent; core-only callers do not use this flow. */
export async function startRuntimeInMode(
  mode: RuntimeStartMode,
  context: StartContext,
): Promise<StartResult & { refreshError?: unknown }> {
  const initial = context.state();
  if (initial.busy) return { kind: "skipped" };
  if (!initial.hasProfile) return { kind: "needs-profile" };
  if (!initial.settings || !canStartRuntime(initial.runtime)) return { kind: "skipped" };

  let previousMode = initial.settings.networkMode;
  let resolvedMode: NetworkMode = previousMode;
  let modeChanged = false;
  let result: StartResult = { kind: "skipped" };
  let refreshError: unknown;
  // Set synchronously before the first await, including preflight and rollback.
  context.setBusy(true);
  try {
    const current = await context.readRuntime();
    context.setRuntime(current);
    // Stale UI must not restart or stop an existing session.
    if (canStartRuntime(current)) {
      const settings = await context.readSettings();
      previousMode = settings.networkMode;
      resolvedMode = mode === "previous" ? previousMode : mode;
      context.setSettings(settings);
      if (resolvedMode === "tun" && !(await context.ensureTunReady())) {
        result = { kind: "cancelled" };
      } else {
        if (resolvedMode !== previousMode) {
          const settings = await context.setNetworkMode(resolvedMode);
          modeChanged = true;
          context.setSettings(settings);
        }
        const runtime = await context.startActive();
        context.setRuntime(runtime);
        if (runtime.phase !== "running") throw new Error(runtime.lastError || runtime.message || "代理核心未进入运行状态");
        result = { kind: "started" };
      }
    }
  } catch (error) {
    result = { kind: "failed", error, restored: false };
    if (modeChanged) {
      try {
        context.setSettings(await context.setNetworkMode(previousMode));
        result.restored = true;
      } catch (rollbackError) {
        result.rollbackError = rollbackError;
      }
    }
  } finally {
    try { await context.refresh(); }
    catch (error) { refreshError = error; }
    finally { context.setBusy(false); }
  }
  if (result.kind === "started" && !refreshError) {
    const observed = context.state();
    if (observed.runtime?.phase !== "running" || observed.settings?.networkMode !== resolvedMode || (resolvedMode === "system_proxy" && !observed.systemProxyActive)) {
      // The accepted start may already have exited, or external proxy settings
      // may have changed. Do not stop/restart an existing session to fix this.
      result = { kind: "failed", restored: false, error: new Error("启动后未确认核心运行或系统代理接管状态，请刷新核对；未自动停止或重新启动代理") };
    }
  }
  return { ...result, refreshError };
}
