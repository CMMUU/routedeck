import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";

const read = (name) => readFileSync(new URL(`../src/${name}`, import.meta.url), "utf8");
const { subscriptionBytes, subscriptionDate, describeSubscriptionUsage, subscriptionCardMarkup } = await import(`data:text/javascript;base64,${Buffer.from(ts.transpileModule(read("subscription-cards.ts"), { compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.ESNext } }).outputText).toString("base64")}`);
const now = Date.parse("2026-09-07T00:00:00Z");
const GiB = 1024 ** 3;
const sample = (changes = {}) => ({ uploadBytes: 2 * GiB, downloadBytes: 28 * GiB, totalBytes: 100 * GiB, expiresAt: Date.parse("2026-10-07T00:00:00Z") / 1000, ...changes });
const subscription = () => ({ profile: { id: "fixture-only", displayName: "测试订阅", source: { type: "remote_subscription", host: "example.test" }, routingMode: "rule", openaiPolicy: { enabled: false, selectedNodes: [], autoMaintain: false } }, summary: { nodeCount: 12, proxyProviderCount: 2 }, revisionCount: 3, latestFetchedAt: "2026-09-05T00:00:00Z", latestMetadata: { bytes: 4096 }, latestValidation: { valid: true }, active: true, status: { checkedAt: "2026-09-07T00:00:00Z", lastError: null, usage: sample(), usageUpdatedAt: "2026-09-07T00:00:00Z" } });

test("provider upload plus download produce quota and remaining, not response bytes", () => {
  const result = describeSubscriptionUsage(sample(), now);
  assert.equal(result.used, 30 * GiB); assert.equal(result.remaining, 70 * GiB);
  assert.equal(result.progress, 30); assert.equal(result.expired, false);
  const card = subscriptionCardMarkup(subscription(), null, now);
  assert.match(card, /30 GiB/); assert.match(card, /共 100 GiB/); assert.match(card, /4 KiB · 3 个版本/);
});
test("zero is a real usage value while missing, negative and unsafe numbers stay unknown", () => {
  assert.equal(subscriptionBytes(0), "0 B"); assert.equal(subscriptionBytes(undefined), "—");
  for (const value of [null, -1, NaN, Infinity, "1024", Number.MAX_SAFE_INTEGER + 1]) assert.equal(subscriptionBytes(value), "—");
  assert.equal(describeSubscriptionUsage(sample({ uploadBytes: 0, downloadBytes: 0 }), now).progress, 0);
  assert.equal(describeSubscriptionUsage(sample({ downloadBytes: null }), now).used, null);
});
test("unknown or zero allowance never claims unlimited nor draws a made-up percentage", () => {
  for (const totalBytes of [null, 0]) {
    const record = subscription(); record.status.usage.totalBytes = totalBytes;
    const result = describeSubscriptionUsage(record.status.usage, now);
    assert.equal(result.percent, null); assert.equal(result.remaining, null);
    const card = subscriptionCardMarkup(record, null, now);
    assert.doesNotMatch(card, /role="progressbar"|不限量|无限/); assert.match(card, /额度未提供/);
  }
});
test("over-quota usage remains truthful but graphical progress and remaining are bounded", () => {
  const result = describeSubscriptionUsage(sample({ downloadBytes: 120 * GiB }), now);
  assert.equal(result.percent, 122); assert.equal(result.progress, 100); assert.equal(result.remaining, 0); assert.equal(result.exhausted, true);
});
test("expiry uses Unix seconds, zero is unknown, and exact expiration is expired", () => {
  assert.equal(describeSubscriptionUsage(sample({ expiresAt: now / 1000 }), now).expired, true);
  for (const expiresAt of [null, 0, -1, Number.MAX_SAFE_INTEGER]) assert.equal(describeSubscriptionUsage(sample({ expiresAt }), now).expires, "服务商未提供有效时间");
  assert.equal(subscriptionDate("invalid"), "尚未记录");
});
test("checked-at and usage sample time remain distinct after a metadata-less refresh", () => {
  const record = subscription(); record.status.usageUpdatedAt = "2026-09-01T00:00:00Z";
  const card = subscriptionCardMarkup(record, null, now);
  assert.match(card, /本次检查未获取新用量，保留上次采样/);
  assert.match(card, /最近检查/); assert.match(card, /用量采样/);
});
test("historical profiles without usage stay unknown rather than borrowing response bytes", () => {
  const record = subscription(); delete record.status;
  const card = subscriptionCardMarkup(record, null, now);
  assert.match(card, /流量信息未提供/); assert.doesNotMatch(card, /role="progressbar"/);
  assert.match(card, /刷新订阅以获取用量/);
});
test("current configuration nodes and provider counts are not advertised as usable nodes", () => {
  const card = subscriptionCardMarkup(subscription(), null, now);
  assert.match(card, /12 节点 · 2 提供器/); assert.doesNotMatch(card, /14 个可用|连接成功|健康|无限/);
  assert.match(card, /已选用/); assert.match(card, /配置校验通过/);
});
test("host/name/id and error content are escaped; no remote thumbnails or fake subscription URL", () => {
  const record = subscription(); record.profile.displayName = '<img src=x onerror="alert(1)">'; record.profile.id = '" onclick="bad';
  record.profile.source.host = '<script>bad</script>'; record.status.lastError = '<b>HTTP 403</b>';
  const card = subscriptionCardMarkup(record, null, now);
  assert.doesNotMatch(card, /<img|<script| onclick="|https:\/\/|token=/);
  assert.match(card, /&lt;img/); assert.match(card, /刷新失败 · &lt;b&gt;HTTP 403/);
});
test("all real subscription actions remain available and selecting an active profile is disabled", () => {
  const card = subscriptionCardMarkup(subscription(), null, now);
  for (const action of ["refresh", "activate", "versions", "delete", "openai-generate"]) assert.match(card, new RegExp(`data-subscription-action="${action}"`));
  assert.match(card, /data-subscription-action="activate"[^>]*disabled/);
  assert.match(card, /<details class="subscription-more"/);
  const main = read("main.ts"); assert.match(main, /title: "删除订阅"/); assert.match(main, /当前订阅正在使用，请先激活其他订阅后再删除/);
});
test("running disaster recovery generation keeps cancel action and blocks competing generation", () => {
  const task = { profileId: "fixture-only", running: true, completed: 1, total: 3 };
  assert.match(subscriptionCardMarkup(subscription(), task, now), /data-subscription-action="openai-cancel"/);
  assert.match(subscriptionCardMarkup(subscription(), { ...task, profileId: "other" }, now), /data-subscription-action="openai-generate"[^>]*disabled/);
});
test("compact cards use glass tokens, readable type and responsive columns without tiny labels", () => {
  const css = read("subscription-cards.css");
  assert.match(css, /repeat\(2, minmax\(0,1fr\)\)/); assert.match(css, /@container subscriptions/);
  assert.match(css, /prefers-reduced-transparency/); assert.match(css, /forced-colors/);
  assert.doesNotMatch(css, /font-size:\s*(?:[0-9]|1[012])px/);
});
