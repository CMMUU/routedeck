"use strict";

// This website is static. It never invokes desktop APIs, opens local listeners,
// accesses a subscription, saves a proxy configuration, or connects to Codex.
(() => {
  const menuButton = document.querySelector(".menu-toggle");
  const menu = document.querySelector("#main-nav");
  function setMenu(open, returnFocus = false) {
    if (!menu || !menuButton) return;
    menu.classList.toggle("is-open", open);
    menuButton.setAttribute("aria-expanded", String(open));
    menuButton.setAttribute("aria-label", open ? "关闭导航" : "打开导航");
    if (returnFocus) menuButton.focus();
  }
  menuButton?.addEventListener("click", () => setMenu(menuButton.getAttribute("aria-expanded") !== "true"));
  menu?.addEventListener("click", event => { if (event.target.closest("a")) setMenu(false); });
  document.addEventListener("keydown", event => {
    if (event.key === "Escape" && menuButton?.getAttribute("aria-expanded") === "true") setMenu(false, true);
  });
  document.addEventListener("click", event => {
    if (menuButton?.getAttribute("aria-expanded") === "true" && !event.target.closest(".site-header")) setMenu(false);
  });
  window.matchMedia("(min-width: 641px)").addEventListener("change", event => { if (event.matches) setMenu(false); });

  const tabs = [...document.querySelectorAll("[data-system]")];
  const architectureButtons = [...document.querySelectorAll("[data-architecture]")];
  if (!tabs.length) return;
  const systems = {
    windows: { name: "Windows 10 / 11", short: "Windows", icon: "windows", format: "EXE", defaultArchitecture: "x64" },
    macos: { name: "macOS", short: "macOS", icon: "apple", format: "DMG", defaultArchitecture: "arm64" },
    linux: { name: "Linux · AppImage", short: "Linux", icon: "linux", format: "AppImage", defaultArchitecture: "x64" },
  };
  // Verified release snapshot, 2026-09-08 03:39 UTC. All six GitHub main packages
  // returned HTTP 200; only Linux ARM64 AppImage did on Gitee, with matching size.
  // A source version or tag must never be used as evidence of a released file.
  const release = {
    version: "v0.7.5",
    files: {
      windows: { x64: "RouteDeck_0.7.5_x64-setup.exe", arm64: "RouteDeck_0.7.5_arm64-setup.exe" },
      macos: { x64: "RouteDeck_0.7.5_x64.dmg", arm64: "RouteDeck_0.7.5_aarch64.dmg" },
      linux: { x64: "RouteDeck_0.7.5_amd64.AppImage", arm64: "RouteDeck_0.7.5_aarch64.AppImage" },
    },
    domesticAvailable: new Set(["linux:arm64"]),
  };
  let selectedSystem = "windows";
  let selectedArchitecture = "x64";
  function updateDownloadSelection() {
    const system = systems[selectedSystem];
    const architecture = selectedArchitecture === "arm64" ? "ARM64" : "x64";
    document.querySelector("#platform-name").textContent = system.name;
    document.querySelector("#platform-icon use").setAttribute("href", `#i-${system.icon}`);
    document.querySelector("#download-panel").setAttribute("aria-labelledby", `tab-${selectedSystem}`);
    tabs.forEach(tab => {
      const active = tab.dataset.system === selectedSystem;
      tab.disabled = false;
      tab.setAttribute("aria-selected", String(active));
      tab.tabIndex = active ? 0 : -1;
    });
    architectureButtons.forEach(button => {
      button.disabled = false;
      button.setAttribute("aria-pressed", String(button.dataset.architecture === selectedArchitecture));
    });
    const filename = release.files[selectedSystem][selectedArchitecture];
    const domesticAvailable = release.domesticAvailable.has(`${selectedSystem}:${selectedArchitecture}`);
    const github = document.querySelector("#download-github");
    const gitee = document.querySelector("#download-gitee");
    github.href = `https://github.com/CMMUU/routedeck/releases/download/${release.version}/${filename}`;
    gitee.href = domesticAvailable
      ? `https://gitee.com/cmmuu/routedeck/releases/download/${release.version}/${filename}`
      : `https://gitee.com/cmmuu/routedeck/releases/tag/${release.version}`;
    github.querySelector("span").textContent = domesticAvailable ? "GitHub 备用" : `GitHub 下载 · ${system.format}`;
    gitee.querySelector("span").textContent = domesticAvailable ? `国内下载 · ${system.format}` : "查看 Gitee 发布";
    github.classList.toggle("button-primary", !domesticAvailable);
    github.classList.toggle("button-secondary", domesticAvailable);
    gitee.classList.toggle("button-primary", domesticAvailable);
    gitee.classList.toggle("button-secondary", !domesticAvailable);
    const links = document.querySelector(".release-links");
    // Keep DOM, visual and keyboard order aligned with the available channel.
    links.append(...(domesticAvailable ? [gitee, github] : [github, gitee]));
    for (const [link, channelName] of [[gitee, "Gitee"], [github, "GitHub"]]) {
      link.dataset.system = selectedSystem;
      link.dataset.architecture = selectedArchitecture;
      link.setAttribute("aria-label", link === gitee && !domesticAvailable
        ? `查看 Gitee ${release.version} 发布页；当前未提供 ${system.short} ${architecture} ${system.format} 安装包`
        : `从 ${channelName} 下载 RouteDeck ${release.version} ${system.short} ${architecture} ${system.format} 安装包`);
    }
    const note = domesticAvailable
      ? `国内渠道已提供 ${system.short} ${architecture} ${system.format} 安装包，GitHub 备用。`
      : `国内镜像暂缺 ${system.short} ${architecture} ${system.format} 安装包，请使用 GitHub。`;
    document.querySelector("#channel-note").textContent = note;
    document.querySelector("#download-selection").textContent = `已选择 ${system.short} ${architecture}，${system.format} 安装包，${release.version}。${note}`;
  }
  function selectSystem(system, moveFocus = false) {
    if (!Object.hasOwn(systems, system)) return;
    selectedSystem = system;
    selectedArchitecture = systems[system].defaultArchitecture;
    updateDownloadSelection();
    if (moveFocus) document.querySelector(`#tab-${system}`).focus();
  }
  tabs.forEach((tab, index) => {
    tab.addEventListener("click", () => selectSystem(tab.dataset.system));
    tab.addEventListener("keydown", event => {
      let next;
      if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
      else if (event.key === "ArrowLeft") next = (index + tabs.length - 1) % tabs.length;
      else if (event.key === "Home") next = 0;
      else if (event.key === "End") next = tabs.length - 1;
      else return;
      event.preventDefault();
      selectSystem(tabs[next].dataset.system, true);
    });
  });
  architectureButtons.forEach(button => button.addEventListener("click", () => {
    if (!["x64", "arm64"].includes(button.dataset.architecture)) return;
    selectedArchitecture = button.dataset.architecture;
    updateDownloadSelection();
  }));
  document.querySelectorAll("[data-select-windows]").forEach(link => link.addEventListener("click", () => selectSystem("windows")));
  updateDownloadSelection();
})();
