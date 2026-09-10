#!/usr/bin/env python3
"""Normalize release DISPLAY metadata only. Never rename/replace/delete a binary or tag."""
import argparse
import os
import re
from urllib.parse import quote

REPO = "/repos/CMMUU/serylane"
GITEE = "/repos/cmmuu/serylane"
BRAND = r"(?:Serylane|RouteDeck|mihomo-codex)"
NOTICE = "发布页统一使用 Serylane 展示名称。历史安装包内部名称、真实文件名、校验值及签名保持原样；本次展示调整不代表重新打包或新增功能。"


def asset_label(name):
    if re.fullmatch(BRAND + r"-LICENSE\.txt", name, re.I):
        return "Serylane · 应用许可证"
    match = re.fullmatch(BRAND + r"[_-](\d+\.\d+\.\d+)[_-](.+)", name, re.I)
    if not match:
        return None  # Third-party core/source/license names must remain accurate.
    version, suffix = match.groups()
    signature = suffix.endswith(".sig")
    if signature:
        suffix = suffix[:-4]
    arm = any(part in suffix for part in ("aarch64", "arm64"))
    if re.fullmatch(r'(?:x64|aarch64)\.(?:dmg|app\.tar\.gz)', suffix):
        platform = "macOS · Apple 芯片（M 系列）" if arm else "macOS · Intel 芯片"
        kind = "DMG 安装包" if suffix.endswith(".dmg") else "应用内更新归档"
    elif re.fullmatch(r'(?:x64|arm64)(?:-setup\.exe|_en-US\.msi)', suffix):
        platform = "Windows · " + ("ARM64" if arm else "x64")
        kind = "EXE 安装包" if suffix.endswith(".exe") else "MSI 安装包"
    elif re.fullmatch(r'(?:(?:aarch64|amd64)\.AppImage|(?:amd64|arm64)\.deb|1\.(?:aarch64|x86_64)\.rpm)', suffix):
        platform = "Linux · " + ("ARM64" if arm else "x64")
        kind = suffix.rsplit(".", 1)[1]
    else:
        return None
    return f"Serylane v{version} · {platform} · {kind}" + (" · 更新签名" if signature else "")


def display_body(body):
    # Canonical repository links change; version paths and filenames do not.
    body = re.sub(r"https://github\.com/CMMUU/(?:mihomo-codex|routedeck)(?=/)", "https://github.com/CMMUU/serylane", body, flags=re.I)
    body = re.sub(r"https://gitee\.com/cmmuu/(?:mihomo-codex|routedeck)(?=/)", "https://gitee.com/cmmuu/serylane", body, flags=re.I)
    lines = []
    fence = None
    for line in body.splitlines():
        if line.lstrip().startswith(('```', '~~~')):
            marker = line.lstrip()[:3]
            fence = None if fence == marker else marker if fence is None else fence
            lines.append(line)
            continue
        if fence:
            lines.append(line)
            continue
        if line.startswith("继续使用 Serylane 展示品牌及蓝色 S 图标。"):
            line = "继续使用 Serylane 展示品牌及蓝色 S 图标。该历史版本的安装身份、程序文件、数据目录、安装包命名和更新签名公钥保持兼容；实际文件名以附件为准。"
        elif line.startswith("- RouteDeck 的展示品牌改为"):
            line = "- 展示品牌统一为 **Serylane — 开源 Mihomo 桌面代理客户端**。更新窗口标题、侧栏、托盘和提示，保留中文界面与已有功能。"
        elif line.startswith("- README 保留 RouteDeck"):
            line = "- 更新 README 项目介绍，保留已有用户的升级兼容说明。"
        elif line.startswith("- 安装名称暂保留"):
            line = "- 本历史版本保留既有安装和内部包身份；安装器、卸载列表及部分系统界面可能仍显示历史名称，不因发布页展示调整而改变。"
        elif line.startswith("- 保留现有 GitHub/Gitee 仓库与更新清单"):
            line = "- 保留更新清单、安装包真实文件名及签名公钥；继续同版本国内优先、GitHub 备用。历史包、签名与标签不重写。"
        elif line.startswith("首个 GitHub 私有仓库版本"):
            line = "首个 GitHub 私有仓库发布阶段的历史版本，完成当时的项目与应用命名整理。"
        elif line.startswith("- 统一项目、应用、窗口、菜单、托盘和主程序名称为"):
            line = "- 统一当时的项目、应用、窗口、菜单、托盘和主程序命名。"
        # Keep executable/config identifiers and code fences untouched; relabel
        # actual installer links without inventing a renamed download target.
        file_match = re.match(r"- `(mihomo-codex_(\d+\.\d+\.\d+)_aarch64\.dmg)`", line)
        if file_match:
            name, version = file_match.groups()
            line = line.replace(f"`{name}`", f"[Serylane v{version} · macOS Apple 芯片（M 系列）](https://github.com/CMMUU/serylane/releases/download/v{version}/{quote(name)})")
        parts = re.split(r"(`[^`]*`|https?://[^\s)]+)", line)
        line = "".join(part if i % 2 else re.sub(r"RouteDeck|mihomo-codex", "Serylane", part, flags=re.I) for i, part in enumerate(parts))
        lines.append(line)
    result = "\n".join(lines).strip()
    if NOTICE not in result:
        result += "\n\n---\n\n" + NOTICE
    return result + "\n"


def immutable_asset(asset):
    return tuple(asset.get(k) for k in ("id", "name", "size", "digest", "state", "browser_download_url", "created_at"))


def normalize(gh, ge=None, apply=False):
    if gh.request(REPO)["id"] != 1355770287:
        raise ValueError("Unexpected GitHub repository identity")
    if ge and ge.request(GITEE)["id"] != 50078322:
        raise ValueError("Unexpected Gitee repository identity")
    releases = gh.pages(REPO + "/releases")
    targets = ge.pages(GITEE + "/releases") if ge else []
    for release in releases:
        tag = release.get("tag_name", "")
        if release.get("draft") or release.get("prerelease") or not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
            continue
        endpoint = f"{REPO}/releases/{int(release['id'])}"
        assets = gh.pages(endpoint + "/assets")
        metadata = {"name": f"Serylane {tag}", "body": display_body(release.get("body") or "")}
        if any(release.get(k) != v for k, v in metadata.items()):
            print(f"{'Apply' if apply else 'Preview'} display: {tag}", flush=True)
            if apply:
                fresh = gh.request(endpoint)
                if any(fresh.get(k) != release.get(k) for k in ("name", "body", "tag_name", "draft", "prerelease", "target_commitish")):
                    raise ValueError("Release changed since inspection; no overwrite")
                gh.request(endpoint, "PATCH", metadata)
                confirmed = gh.request(endpoint)
                if any(confirmed.get(k) != v for k, v in metadata.items()) or any(confirmed.get(k) != release.get(k) for k in ("tag_name", "draft", "prerelease", "target_commitish", "published_at")):
                    raise ValueError("Release metadata update not confirmed")
        for asset in assets:
            label = asset_label(asset["name"])
            if label and label != asset.get("label"):
                print(f"{'Apply' if apply else 'Preview'} label: {label}", flush=True)
                if apply:
                    path = f"{REPO}/releases/assets/{int(asset['id'])}"
                    fresh = gh.request(path)
                    if immutable_asset(fresh) != immutable_asset(asset) or fresh.get('label') != asset.get('label'):
                        raise ValueError("Asset changed since inspection; no overwrite")
                    gh.request(path, "PATCH", {"label": label})
                    confirmed = gh.request(path)
                    if confirmed.get("label") != label or immutable_asset(confirmed) != immutable_asset(asset):
                        raise ValueError("Asset display update not verified")
        # Gitee has no equivalent independent attachment display label. Update
        # existing release headings/body only, including retired historical rows.
        matching = [row for row in targets if row.get("tag_name") == tag]
        if len(matching) > 1:
            raise ValueError("Duplicate Gitee release tag")
        if matching and any((matching[0].get(k) or '').replace('\r\n', '\n') != v for k, v in metadata.items()):
            print(f"{'Apply' if apply else 'Preview'} Gitee display: {tag}", flush=True)
            if apply:
                path = f"{GITEE}/releases/{int(matching[0]['id'])}"
                fresh = ge.request(path)
                protected = ('tag_name', 'target_commitish', 'prerelease', 'created_at')
                if any(fresh.get(k) != matching[0].get(k) for k in ('name', 'body', *protected)):
                    raise ValueError('Gitee release changed since inspection')
                # Gitee requires tag_name even for display-only PATCH. Echo the
                # verified existing tag and prerelease state; never retarget it.
                ge.request(path, "PATCH", {**metadata, 'tag_name': fresh['tag_name'],
                                           'prerelease': str(bool(fresh.get('prerelease'))).lower()})
                confirmed = ge.request(path)
                if (any((confirmed.get(k) or '').replace('\r\n', '\n') != v for k, v in metadata.items())
                        or any(confirmed.get(k) != fresh.get(k) for k in protected)):
                    raise ValueError("Gitee display update not confirmed")
        if apply:
            after = gh.pages(endpoint + '/assets')
            if sorted(map(immutable_asset, after)) != sorted(map(immutable_asset, assets)):
                raise ValueError('Asset set changed during display update')
    print("Release display verified; binary names, bytes, signatures and tags were not rewritten.", flush=True)


def main():
    from publish_github_release import GitHub
    from sync_gitee import Api
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply', action='store_true')
    parser.add_argument('--gitee', action='store_true')
    args = parser.parse_args()
    class DisplayGitHub(GitHub):
        def request(self, path, method='GET', data=None):
            if method != 'GET':
                if method != 'PATCH' or not re.fullmatch(REPO + r'/releases/(?:assets/)?[1-9]\d*', path):
                    raise ValueError('Only display metadata PATCH is allowed')
                expected = {'label'} if '/assets/' in path else {'name', 'body'}
                if set(data) != expected:
                    raise ValueError('No binary, tag or publication mutations allowed')
            return self.api(path, method, data)
    gh = DisplayGitHub()
    ge = Api('gitee', os.environ['GITEE_TOKEN']) if args.gitee else None
    normalize(gh, ge, args.apply)


if __name__ == '__main__':
    main()
