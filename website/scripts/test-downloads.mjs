import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { runInNewContext } from 'node:vm';

const html = await readFile(new URL('../public/index.html', import.meta.url), 'utf8');
const script = await readFile(new URL('../public/site.js', import.meta.url), 'utf8');

// Independent, publicly verified 2026-09-08 release snapshot. These tests are
// offline: changing the release requires checking the remote assets again.
const version = 'v0.7.4';
const githubBase = `https://github.com/CMMUU/routedeck/releases/download/${version}/`;
const giteeBase = `https://gitee.com/cmmuu/routedeck/releases/download/${version}/`;
const giteeRelease = `https://gitee.com/cmmuu/routedeck/releases/tag/${version}`;
const downloads = [
  { system: 'windows', architecture: 'x64', short: 'Windows', name: 'Windows 10 / 11', icon: 'windows', format: 'EXE', filename: 'RouteDeck_0.7.4_x64-setup.exe', domestic: true },
  { system: 'windows', architecture: 'arm64', short: 'Windows', name: 'Windows 10 / 11', icon: 'windows', format: 'EXE', filename: 'RouteDeck_0.7.4_arm64-setup.exe', domestic: true },
  { system: 'macos', architecture: 'x64', short: 'macOS', name: 'macOS', icon: 'apple', format: 'DMG', filename: 'RouteDeck_0.7.4_x64.dmg', domestic: true },
  { system: 'macos', architecture: 'arm64', short: 'macOS', name: 'macOS', icon: 'apple', format: 'DMG', filename: 'RouteDeck_0.7.4_aarch64.dmg', domestic: true },
  { system: 'linux', architecture: 'x64', short: 'Linux', name: 'Linux · AppImage', icon: 'linux', format: 'AppImage', filename: 'RouteDeck_0.7.4_amd64.AppImage', domestic: true },
  { system: 'linux', architecture: 'arm64', short: 'Linux', name: 'Linux · AppImage', icon: 'linux', format: 'AppImage', filename: 'RouteDeck_0.7.4_aarch64.AppImage', domestic: true },
];

// Read actual markup, rather than duplicating its attributes in a fake fixture.
// This deliberately supports only the small DOM surface used by site.js; an
// unexpected selector fails instead of silently masking a production change.
const tags = [...html.matchAll(/<([a-z][\w-]*)\b([^<>]*)>/gi)].map(([, tag, source]) => ({
  tag,
  attributes: Object.fromEntries([...source.matchAll(/([^\s=/'">]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+)))?/g)]
    .map(([, key, double, single, bare]) => [key, double ?? single ?? bare ?? ''])),
}));
const tagged = predicate => tags.filter(predicate);
function oneTag(predicate) {
  const found = tagged(predicate);
  assert.equal(found.length, 1, 'Expected exactly one matching HTML element');
  return found[0];
}
const byId = id => oneTag(tag => tag.attributes.id === id);
function contentById(id) {
  const { tag } = byId(id);
  const match = html.match(new RegExp(`<${tag}\\b[^>]*\\bid="${id}"[^>]*>([\\s\\S]*?)<\\/${tag}>`));
  assert.ok(match, `Missing content for #${id}`);
  return match[1];
}
const hasClass = (tag, name) => (tag.attributes.class ?? '').split(/\s+/).includes(name);
const systemTags = tagged(tag => Object.hasOwn(tag.attributes, 'data-system'));
const architectureTags = tagged(tag => Object.hasOwn(tag.attributes, 'data-architecture'));
const releaseLinks = html.match(/<div\b[^>]*class="release-links"[^>]*>([\s\S]*?)<\/div>/)?.[1] ?? '';

function createPage(source = script) {
  const page = { activeElement: null };
  function element({ tag = 'span', attributes = {} } = {}) {
    const attrs = new Map(Object.entries(attributes));
    const classes = new Set((attributes.class ?? '').split(/\s+/).filter(Boolean));
    const listeners = new Map();
    return {
      tag, children: [], descendants: new Map(),
      dataset: Object.fromEntries(Object.entries(attributes).filter(([key]) => key.startsWith('data-'))
        .map(([key, value]) => [key.slice(5).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()), value])),
      disabled: Object.hasOwn(attributes, 'disabled'),
      tabIndex: Number(attributes.tabindex ?? 0),
      textContent: '',
      get href() { return attrs.get('href') ?? ''; },
      set href(value) { attrs.set('href', value); },
      getAttribute: name => attrs.get(name) ?? null,
      setAttribute: (name, value) => attrs.set(name, String(value)),
      classList: {
        contains: name => classes.has(name),
        toggle(name, force = !classes.has(name)) { if (force) classes.add(name); else classes.delete(name); return force; },
      },
      addEventListener(type, handler) { listeners.set(type, [...(listeners.get(type) ?? []), handler]); },
      dispatch(type, details = {}) {
        const event = { target: this, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...details };
        for (const handler of listeners.get(type) ?? []) handler(event);
        return event;
      },
      focus() { page.activeElement = this; },
      querySelector(selector) { assert.ok(this.descendants.has(selector), `Unexpected child selector: ${selector}`); return this.descendants.get(selector); },
      append(...nodes) { this.children = [...this.children.filter(child => !nodes.includes(child)), ...nodes]; },
    };
  }
  const selectors = new Map();
  for (const id of ['main-nav', 'platform-name', 'download-panel', 'download-github', 'download-gitee', 'channel-note', 'download-selection']) {
    selectors.set(`#${id}`, element(byId(id)));
  }
  selectors.set('.menu-toggle', element(oneTag(tag => hasClass(tag, 'menu-toggle'))));
  selectors.set('.release-links', element(oneTag(tag => hasClass(tag, 'release-links'))));
  selectors.set('#platform-icon use', element());
  const tabs = systemTags.map(element);
  const architectures = architectureTags.map(element);
  const windowsLinks = tagged(tag => Object.hasOwn(tag.attributes, 'data-select-windows')).map(element);
  tabs.forEach(tab => selectors.set(`#${tab.getAttribute('id')}`, tab));
  const query = selector => {
    assert.ok(selectors.has(selector), `Unexpected document selector: ${selector}`);
    return selectors.get(selector);
  };
  const github = query('#download-github');
  const gitee = query('#download-gitee');
  for (const link of [github, gitee]) link.descendants.set('span', element());
  query('.release-links').children = [github, gitee].sort((a, b) =>
    releaseLinks.indexOf(`id="${a.getAttribute('id')}"`) - releaseLinks.indexOf(`id="${b.getAttribute('id')}"`));
  const collections = new Map([['[data-system]', tabs], ['[data-architecture]', architectures], ['[data-select-windows]', windowsLinks]]);
  const document = Object.assign(element(), {
    querySelector: query,
    querySelectorAll(selector) { assert.ok(collections.has(selector), `Unexpected collection: ${selector}`); return collections.get(selector); },
  });
  const forbidden = () => assert.fail('The static website must not call network, storage or desktop APIs');
  const window = { matchMedia: () => ({ addEventListener() {} }), open: forbidden, fetch: forbidden };
  for (const name of ['__TAURI__', 'localStorage', 'sessionStorage']) Object.defineProperty(window, name, { get: forbidden });
  runInNewContext(source, {
    document, window, fetch: forbidden, XMLHttpRequest: forbidden, WebSocket: forbidden,
    EventSource: forbidden, navigator: { sendBeacon: forbidden },
  }, { filename: 'website/public/site.js', timeout: 1_000 });
  return { page, query, tabs, architectures, windowsLinks, github, gitee };
}

function assertSelection(view, expected) {
  const { system, architecture, short, name, icon, format, filename, domestic } = expected;
  const label = `${short} ${architecture === 'arm64' ? 'ARM64' : 'x64'} ${format} 安装包`;
  assert.equal(view.github.href, githubBase + filename, `${system}/${architecture}: exact GitHub asset`);
  assert.equal(view.gitee.href, domestic ? giteeBase + filename : giteeRelease, `${system}/${architecture}: verified Gitee asset or release fallback`);
  assert.equal(view.query('#platform-name').textContent, name);
  assert.equal(view.query('#platform-icon use').getAttribute('href'), `#i-${icon}`);
  assert.equal(view.query('#download-panel').getAttribute('aria-labelledby'), `tab-${system}`);
  for (const tab of view.tabs) {
    const selected = tab.dataset.system === system;
    assert.equal(tab.disabled, false);
    assert.equal(tab.getAttribute('aria-selected'), String(selected));
    assert.equal(tab.tabIndex, selected ? 0 : -1, 'Only selected system is in the Tab sequence');
  }
  for (const button of view.architectures) {
    assert.equal(button.disabled, false);
    assert.equal(button.getAttribute('aria-pressed'), String(button.dataset.architecture === architecture));
  }
  const primary = domestic ? view.gitee : view.github;
  const secondary = domestic ? view.github : view.gitee;
  assert.deepEqual(view.query('.release-links').children, [primary, secondary], 'Primary action comes first in DOM and keyboard order');
  for (const link of [view.github, view.gitee]) {
    assert.equal(link.classList.contains('button-primary'), link === primary);
    assert.equal(link.classList.contains('button-secondary'), link === secondary);
    assert.equal(link.dataset.system, system);
    assert.equal(link.dataset.architecture, architecture);
    assert.equal(link.getAttribute('target'), '_blank');
    assert.match(link.getAttribute('rel'), /\bnoopener\b/);
    assert.match(link.getAttribute('rel'), /\bnoreferrer\b/);
  }
  assert.equal(view.github.querySelector('span').textContent, domestic ? 'GitHub 备用' : `GitHub 下载 · ${format}`);
  assert.equal(view.gitee.querySelector('span').textContent, domestic ? `国内下载 · ${format}` : '查看 Gitee 发布');
  assert.equal(view.github.getAttribute('aria-label'), `从 GitHub 下载 RouteDeck ${version} ${label}`);
  assert.equal(view.gitee.getAttribute('aria-label'), domestic
    ? `从 Gitee 下载 RouteDeck ${version} ${label}`
    : `查看 Gitee ${version} 发布页；当前未提供 ${label}`);
  const note = domestic ? `国内渠道已提供 ${label}，GitHub 备用。` : `国内镜像暂缺 ${label}，请使用 GitHub。`;
  assert.equal(view.query('#channel-note').textContent, note, 'Missing mirror notice names the specific package format');
  assert.equal(view.query('#download-selection').textContent,
    `已选择 ${short} ${architecture === 'arm64' ? 'ARM64' : 'x64'}，${format} 安装包，${version}。${note}`);
}

let checks = 0;
function check(name, run) { run(); checks++; console.log(`PASS: ${name}`); }
const expectedSelection = (system, architecture) => downloads.find(item => item.system === system && item.architecture === architecture);

check('No-JavaScript HTML offers domestic Windows x64 first, GitHub backup and disabled selectors', () => {
  assert.deepEqual(systemTags.map(tag => tag.attributes['data-system']), ['windows', 'macos', 'linux']);
  assert.deepEqual(architectureTags.map(tag => tag.attributes['data-architecture']), ['x64', 'arm64']);
  for (const tag of [...systemTags, ...architectureTags]) {
    assert.equal(tag.tag, 'button');
    assert.ok(Object.hasOwn(tag.attributes, 'disabled'));
  }
  assert.equal(byId('tab-windows').attributes['aria-selected'], 'true');
  for (const id of ['tab-macos', 'tab-linux']) {
    assert.equal(byId(id).attributes['aria-selected'], 'false');
    assert.equal(byId(id).attributes.tabindex, '-1');
  }
  assert.equal(architectureTags[0].attributes['aria-pressed'], 'true');
  assert.equal(architectureTags[1].attributes['aria-pressed'], 'false');
  assert.equal(byId('download-github').attributes.href, githubBase + downloads[0].filename);
  assert.equal(byId('download-gitee').attributes.href, giteeBase + downloads[0].filename);
  assert.ok(hasClass(byId('download-gitee'), 'button-primary'));
  assert.ok(hasClass(byId('download-github'), 'button-secondary'));
  assert.deepEqual([...releaseLinks.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]), ['download-gitee', 'download-github']);
  assert.equal(byId('download-gitee').attributes['aria-label'], `从 Gitee 下载 RouteDeck ${version} Windows x64 EXE 安装包`);
  assert.match(contentById('download-gitee'), /<span>国内下载 · EXE<\/span>/);
  assert.match(contentById('download-github'), /<span>GitHub 备用<\/span>/);
  assert.equal(contentById('platform-name'), 'Windows 10 / 11');
  assert.match(contentById('platform-icon'), /<use href="#i-windows"/);
  assert.equal(byId('download-panel').attributes['aria-labelledby'], 'tab-windows');
  assert.equal(byId('download-selection').attributes['aria-live'], 'polite');
  assert.match(contentById('download-selection'), /Windows x64，EXE 安装包/);
  assert.ok(contentById('download-selection').includes(version));
  const fallback = html.match(/<noscript>([\s\S]*?)<\/noscript>/)?.[1] ?? '';
  assert.match(fallback, /Windows x64/);
  assert.match(fallback, /JavaScript/);
  assert.ok(fallback.includes(`href="https://github.com/CMMUU/routedeck/releases/tag/${version}"`));
  assert.equal(contentById('channel-note'), '国内渠道已提供 Windows x64 EXE 安装包，GitHub 备用。');
});

check('Both route illustrations are off, unattached and non-interactive', () => {
  const previews = [...html.matchAll(/<figure\b([^>]*)>([\s\S]*?)<\/figure>/g)]
    .filter(([, attributes]) => /class="(?:app-preview|route-preview)"/.test(attributes));
  assert.equal(previews.length, 2);
  for (const [, , content] of previews) {
    assert.match(content, /已关闭/);
    assert.match(content, /未接入/);
    assert.match(content, /界面示意 · 非实时状态/);
    assert.doesNotMatch(content, /<(?:a|button|input|select|textarea|form)\b|\btabindex\s*=|\bon\w+\s*=|role="(?:button|switch|checkbox)"/i);
    assert.match(content, /class="illustrated-switch" aria-hidden="true"/);
  }
  assert.match(html, /保存设置不会启动服务，也不会接入 Codex。/);
  assert.doesNotMatch(script, /\b(?:fetch|XMLHttpRequest|WebSocket|EventSource|invoke|sendBeacon)\s*\(|__TAURI__/);
});

check('Six platform/architecture combinations use exact verified URLs, labels and primary DOM order', () => {
  const view = createPage();
  assertSelection(view, downloads[0]);
  for (const expected of downloads) {
    view.query(`#tab-${expected.system}`).dispatch('click');
    view.architectures.find(button => button.dataset.architecture === expected.architecture).dispatch('click');
    assertSelection(view, expected);
  }
});

check('Simulated missing domestic packages still fall back to exact GitHub assets and format-specific notices', () => {
  // Alter only the in-memory fixture, not the shipped code or its public API.
  // Keep testing the fallback even when all packages in this snapshot exist.
  const availability = /domesticAvailable:\s*new Set\(\[[\s\S]*?\]\)/g;
  assert.equal([...script.matchAll(availability)].length, 1, 'Expected one release availability fixture');
  for (const available of [[], ['windows:arm64', 'macos:arm64']]) {
    const source = script.replace(availability, `domesticAvailable: new Set(${JSON.stringify(available)})`);
    const view = createPage(source);
    assertSelection(view, { ...downloads[0], domestic: false });
    for (const expected of downloads) {
      view.query(`#tab-${expected.system}`).dispatch('click');
      view.architectures.find(button => button.dataset.architecture === expected.architecture).dispatch('click');
      assertSelection(view, { ...expected, domestic: available.includes(`${expected.system}:${expected.architecture}`) });
    }
  }
});

check('System selection resets to sensible defaults; the Windows hero action resets both selections', () => {
  const view = createPage();
  for (const [system, architecture] of [['macos', 'arm64'], ['linux', 'x64'], ['windows', 'x64'], ['macos', 'arm64']]) {
    view.architectures.find(button => button.dataset.architecture === 'arm64').dispatch('click');
    view.query(`#tab-${system}`).dispatch('click');
    assertSelection(view, expectedSelection(system, architecture));
  }
  assert.ok(view.windowsLinks.length > 0);
  for (const link of view.windowsLinks) {
    view.query('#tab-linux').dispatch('click');
    view.architectures.find(button => button.dataset.architecture === 'arm64').dispatch('click');
    link.dispatch('click');
    assertSelection(view, downloads[0]);
  }
});

check('Arrow/Home/End keys wrap and move focus; Tab and unrelated keys retain native behavior', () => {
  const view = createPage();
  let active = view.query('#tab-windows');
  for (const [key, system, architecture] of [
    ['ArrowLeft', 'linux', 'x64'], ['ArrowRight', 'windows', 'x64'],
    ['ArrowRight', 'macos', 'arm64'], ['End', 'linux', 'x64'], ['Home', 'windows', 'x64'],
  ]) {
    assert.equal(active.dispatch('keydown', { key }).defaultPrevented, true);
    active = view.query(`#tab-${system}`);
    assert.equal(view.page.activeElement, active);
    assertSelection(view, expectedSelection(system, architecture));
  }
  for (const key of ['Tab', 'Enter', 'ArrowDown', 'Escape']) {
    assert.equal(active.dispatch('keydown', { key }).defaultPrevented, false);
    assert.equal(view.page.activeElement, active);
    assertSelection(view, downloads[0]);
  }
});

console.log(`PASS: ${checks} offline download/interaction checks; no network, browser or desktop access.`);
