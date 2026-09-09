import type { SessionResumeStatus } from "./types";

export function sessionResumePresentation(status: SessionResumeStatus | null) {
  const phase = status?.phase ?? "idle";
  const busy = phase === "pending" || phase === "restoring";
  return {
    busy,
    visible: busy || phase === "paused",
    title: phase === "paused" ? "上次运行状态暂未恢复" : "正在恢复上次运行状态",
    message: status?.message ?? "尚未读取恢复状态。",
  };
}

export function sessionResumeHelp(launchAtLogin: boolean, restoreLastSession: boolean) {
  if (!restoreLastSession) return "恢复已关闭：打开应用时不会自动启动代理核心。";
  return `${launchAtLogin ? "登录后自动打开应用并恢复" : "未开启登录时启动：重启电脑后，手动打开应用才会恢复"}上次运行状态。上次已停止则保持停止；运行中则使用原模式与选用配置。TUN 权限不足或其他系统代理占用时暂停恢复，不会自动接入 Codex 或启动其他程序。`;
}
