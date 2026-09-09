import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const read = (path) => readFileSync(join(root, path), "utf8").replaceAll("\r\n", "\n");
const json = (path) => JSON.parse(read(path));
const displayName = "Serylane";
// Display/binary names change; data/helper IDs and signed compatibility aliases do not.
const installerName = "Serylane";
const slug = "serylane";
const libraryName = "serylane_lib";
// These identifiers are compatibility contracts, not user-facing branding.
const identifier = "com.cmmuu.mihomodesktop";
const helperName = "mihomo-tun-helper";
const ruleMetadataPrefix = "# mihomo-codex-rule:";
const pkg = json("package.json");
const lock = json("package-lock.json");
const config = json("src-tauri/tauri.conf.json");
const cargo = read("src-tauri/Cargo.toml");
const protocol = read("src-tauri/src/tun_service/protocol.rs");

assert.equal(pkg.name, slug);
assert.equal(lock.name, slug);
assert.equal(lock.packages[""].name, slug);
assert.equal(lock.version, pkg.version);
assert.equal(lock.packages[""].version, pkg.version);
assert.equal(config.productName, installerName);
assert.equal(config.mainBinaryName, "serylane");
assert.equal(config.bundle.windows.wix.upgradeCode, "3dc8b957-21f1-51b9-ab4c-df67ce49b53d");
assert.equal(config.bundle.windows.nsis.template, "installer/installer.nsi");
assert.equal(config.version, pkg.version);
assert.equal(config.identifier, identifier);
assert.equal(config.app.windows.find((window) => window.label === "main").title, displayName);
assert.ok(cargo.includes(`[package]\nname = "${slug}"\nversion = "${pkg.version}"`));
assert.ok(cargo.includes(`default-run = "${slug}"`));
assert.ok(cargo.includes(`version = "${pkg.version}"`));
assert.ok(cargo.includes(`name = "${libraryName}"`));
assert.ok(read("src-tauri/Cargo.lock").includes(`name = "${slug}"\nversion = "${pkg.version}"`));
assert.ok(read("src-tauri/src/main.rs").includes(`${libraryName}::run()`));
assert.ok(read(`src-tauri/src/bin/${helperName}.rs`).includes(`${libraryName}::tun_service::daemon::run()`));
assert.ok(protocol.includes('APP_BINARY_NAME: &str = "serylane"'));
assert.ok(protocol.includes(`LABEL: &str = "${identifier}.tun-helper"`));
assert.ok(protocol.includes(`HELPER_BINARY_NAME: &str = "${helperName}"`));
assert.ok(protocol.includes(`PLIST_NAME: &CStr = c"${identifier}.tun-helper.plist"`));
assert.ok(read("index.html").includes(`<title>${displayName}</title>`));
assert.ok(read("src/main.ts").includes(`<strong>${displayName}</strong>`));
assert.doesNotMatch(read("src/main.ts"), /RouteDeck/, "All user-facing dialogs must use Serylane");
assert.ok(read("src-tauri/src/lib.rs").includes(`tooltip("${displayName}")`));
assert.ok(read("src-tauri/src/lib.rs").includes(`product_name: "${displayName}"`));
assert.ok(read("src-tauri/src/traffic_monitor.rs").includes(`Some("${displayName}")`));
assert.ok(read("tests/fixtures/theme-preview.html").includes(`<title>${displayName} ·`));
assert.ok(read("tests/fixtures/theme-preview.ts").includes(`productName: "${displayName}"`));
const introduction = read("README.md");
assert.match(introduction, /^# Serylane /);
assert.doesNotMatch(introduction, /formerly\s+RouteDeck|(?:原|曾用名)[^。\n]*RouteDeck/i,
  "The project introduction uses Serylane without former-brand marketing copy");
assert.doesNotMatch(cargo.match(/^description\s*=.*$/m)?.[0] ?? "", /RouteDeck/i,
  "Package description uses only the current display brand");

const plist = read(`src-tauri/helper/${identifier}.tun-helper.plist`);
assert.ok(plist.includes(`<string>${identifier}</string>`));
assert.ok(plist.includes(`<string>${identifier}.tun-helper</string>`));
assert.ok(plist.includes(`<string>Contents/MacOS/${helperName}</string>`));
assert.equal(
  config.bundle.macOS.files[`Library/LaunchDaemons/${identifier}.tun-helper.plist`],
  `helper/${identifier}.tun-helper.plist`,
);
assert.ok(read("src-tauri/src/storage.rs").includes(".app_data_dir()"));
assert.ok(read("src-tauri/src/user_rules.rs").includes(`METADATA_PREFIX: &str = "${ruleMetadataPrefix}"`));
assert.ok(read("src/rule-manager.ts").includes(ruleMetadataPrefix));
assert.ok(read("tests/fixtures/theme-preview.ts").includes(ruleMetadataPrefix));

// Existing Codex leases, old updater clients and package managers must survive a
// display-only rename. Do not broaden these exact identities to new-name aliases.
assert.ok(read("src-tauri/src/local_routing/codex.rs").includes(`const PROVIDER: &str = "routedeck"`));
assert.ok(read("src-tauri/src/local_routing/codex.rs").includes('"codex-lease.json"'));
assert.ok(read("src-tauri/src/local_routing/codex.rs").includes(`provider["name"] = value("${displayName} 本地路由")`));
const updater = read("src-tauri/src/app_update.rs");
for (const endpoint of [
  "https://github.com/CMMUU/serylane/releases/latest/download/latest.json",
  "https://gitee.com/api/v5/repos/cmmuu/routedeck/releases/latest",
  "https://github.com/CMMUU/routedeck/releases",
  "https://gitee.com/cmmuu/routedeck/releases",
]) assert.ok(updater.includes(`"${endpoint}"`), `Retain updater endpoint: ${endpoint}`);
assert.ok(updater.includes('format!("RouteDeck_{version}_{suffix}")'));
assert.ok(updater.includes("if url.as_str() != expected && !legacy_github"));
assert.ok(updater.includes('"https://github.com/CMMUU/serylane/releases"'));
for (const path of [".github/workflows/release.yml", ".github/workflows/sync-gitee.yml"]) {
  assert.ok(read(path).includes("github.repository == 'CMMUU/serylane'"));
  assert.ok(!read(path).includes("github.repository == 'CMMUU/routedeck'"));
}
assert.ok(read("scripts/updater_release.py").includes('prefix = f"RouteDeck_{version}"'));
assert.ok(read("scripts/publish_github_release.py").includes('prefix = f"{brand}_{version}"'));
assert.equal(createHash("sha256").update(config.plugins.updater.pubkey).digest("hex"),
  "de516897f5cc1e06aab7fa1e822ce6f6db8ae95547c15b2b216d9c0391c52792", "Retain the existing signed-update verification key");

console.log(`${displayName} ${pkg.version}: display/installer/binary names unified; legacy update/data identities retained`);
