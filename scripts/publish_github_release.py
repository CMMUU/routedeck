#!/usr/bin/env python3
"""Publish the complete six-platform bundle set, without replacing release assets."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from urllib.parse import quote, urlparse
from urllib.request import HTTPRedirectHandler, Request, build_opener

from generate_compliance import INPUT_PATHS, markdown as compliance_markdown, property_value
from updater_release import stage_updaters


REPOSITORY = "CMMUU/routedeck"
MAX_SOURCE_BYTES = 256 * 1024 * 1024


class ReleaseError(Exception):
    pass


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def release_title(tag):
    match = re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag)
    if not match:
        raise ReleaseError("Release title requires an exact stable version tag")
    # Display-only transition. Old tags keep their default title, while already
    # published releases below are never patched, regardless of their branding.
    brand = "Serylane" if tuple(map(int, match.groups())) >= (0, 7, 4) else "RouteDeck"
    return f"{brand} {tag}"


def package_names(version):
    # Tauri CLI 2.11.4 uses productName, not mainBinaryName, for bundle filenames.
    # Linux package metadata is kebab-cased separately; filenames retain the brand.
    prefix = f"RouteDeck_{version}"
    return {
        "macos-aarch64": [f"{prefix}_aarch64.dmg"],
        "macos-x64": [f"{prefix}_x64.dmg"],
        "windows-x64": [f"{prefix}_x64-setup.exe", f"{prefix}_x64_en-US.msi"],
        "windows-arm64": [f"{prefix}_arm64-setup.exe", f"{prefix}_arm64_en-US.msi"],
        "linux-x64": [f"{prefix}_amd64.AppImage", f"{prefix}_amd64.deb",
                      f"RouteDeck-{version}-1.x86_64.rpm"],
        "linux-arm64": [f"{prefix}_aarch64.AppImage", f"{prefix}_arm64.deb",
                        f"RouteDeck-{version}-1.aarch64.rpm"],
    }


def validate_version(root, tag):
    if not re.fullmatch(r"v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", tag):
        raise ReleaseError("Automatic publishing requires a stable vX.Y.Z tag")
    version = tag[1:]
    versions = [json.loads((root / path).read_text(encoding="utf-8"))["version"]
                for path in ("package.json", "src-tauri/tauri.conf.json")]
    versions.append(tomllib.loads((root / "src-tauri/Cargo.toml").read_text(encoding="utf-8"))["package"]["version"])
    npm_lock = json.loads((root / "package-lock.json").read_text(encoding="utf-8"))
    versions.extend([npm_lock["version"], npm_lock["packages"][""]["version"]])
    application = [item for item in tomllib.loads((root / "src-tauri/Cargo.lock").read_text(encoding="utf-8"))["package"] if item["name"] == "routedeck" and not item.get("source")]
    if len(application) != 1:
        raise ReleaseError("Cargo.lock must contain exactly one application package")
    versions.append(application[0]["version"])
    if any(value != version for value in versions):
        raise ReleaseError("Release tag does not match all application versions")
    config = json.loads((root / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    if tuple(map(int, version.split("."))) >= (0, 7, 0) and config.get("bundle", {}).get("createUpdaterArtifacts") is not True:
        raise ReleaseError("RouteDeck 0.7.0 and later require signed updater artifacts")
    notes = root / "docs" / f"发布说明-{tag}.md"
    if not notes.is_file() or not notes.read_text(encoding="utf-8").strip():
        raise ReleaseError(f"Reviewed release notes are required: docs/发布说明-{tag}.md")
    return version, notes.read_text(encoding="utf-8")


def collect_packages(artifacts, version):
    expected = package_names(version)
    if {path.name for path in artifacts.iterdir()} != set(expected):
        raise ReleaseError("Exactly six platform artifact directories are required")
    result = []
    for platform, names in expected.items():
        folder = artifacts / platform
        if not folder.is_dir() or folder.is_symlink():
            raise ReleaseError(f"Invalid artifact directory: {platform}")
        candidates = [path for path in folder.rglob("*")
                      if path.suffix in {".dmg", ".exe", ".msi", ".AppImage", ".deb", ".rpm"}]
        if sorted(path.name for path in candidates) != sorted(names):
            raise ReleaseError(f"Missing, duplicated or unexpected packages in {platform}")
        for path in candidates:
            if not path.is_file() or path.is_symlink() or not path.stat().st_size:
                raise ReleaseError(f"Invalid or empty package: {path.name}")
        result.extend(candidates)
    return result


class SourceRedirects(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        target = urlparse(newurl)
        if target.scheme != "https" or target.hostname not in {"github.com", "codeload.github.com"}:
            raise ReleaseError("Unexpected upstream source redirect")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def download_source(url, destination):
    opener = build_opener(SourceRedirects())
    request = Request(url, headers={"User-Agent": "routedeck-release"})
    total = 0
    with opener.open(request, timeout=120) as response, destination.open("wb") as output:
        while block := response.read(1024 * 1024):
            total += len(block)
            if total > MAX_SOURCE_BYTES:
                raise ReleaseError("Upstream source exceeds the download limit")
            output.write(block)


def generate_compliance(root, output):
    result = subprocess.run(
        [sys.executable, str(root / "scripts/generate_compliance.py"), "--output-dir", str(output)],
        cwd=root, check=False,
    )
    if result.returncode:
        raise ReleaseError("Dependency SBOM generation failed; no release was published")


def validate_compliance(root, output, version):
    package = json.loads((root / "package.json").read_text(encoding="utf-8"))
    npm_lock = json.loads((root / "package-lock.json").read_text(encoding="utf-8"))
    cargo = tomllib.loads((root / "src-tauri/Cargo.toml").read_text(encoding="utf-8"))["package"]
    for source in (package, npm_lock["packages"][""], cargo):
        if source.get("version") != version or source.get("license") != "GPL-3.0-only":
            raise ReleaseError("Application version and GPL-3.0-only license metadata must agree")
    # The checked-in GPL text is pinned by the core manifest and copied verbatim
    # to the project's LICENSE. A short label or placeholder is insufficient.
    if (root / "LICENSE").read_bytes() != (root / "third-party/Mihomo-LICENSE.txt").read_bytes():
        raise ReleaseError("Application GPL license text does not match the complete pinned text")
    for name in ("sbom.cdx.json", "license-inventory.md"):
        path = output / name
        if not path.is_file() or path.is_symlink() or not path.stat().st_size:
            raise ReleaseError(f"Missing generated compliance asset: {name}")
    bom = json.loads((output / "sbom.cdx.json").read_text(encoding="utf-8"))
    component = bom.get("metadata", {}).get("component", {})
    if (bom.get("bomFormat") != "CycloneDX" or bom.get("specVersion") != "1.6"
            or component.get("name") != "routedeck" or component.get("version") != version
            or component.get("licenses") != [{"expression": "GPL-3.0-only"}]):
        raise ReleaseError("Generated SBOM application metadata does not match the release")
    inputs = {path: sha256(root / path) for path in INPUT_PATHS}
    prefix = "routedeck:input-sha256:"
    declared = [item for item in bom["metadata"].get("properties", []) if item["name"].startswith(prefix)]
    recorded = {item["name"][len(prefix):]: item["value"] for item in declared}
    if len(recorded) != len(declared) or recorded != inputs:
        raise ReleaseError("Generated SBOM input checksums do not match the checked-out tag sources")
    cargo_lock = tomllib.loads((root / "src-tauri/Cargo.lock").read_text(encoding="utf-8"))
    core = json.loads((root / "src-tauri/core-manifest.json").read_text(encoding="utf-8"))
    expected = {
        ("cargo", item["name"], item["version"])
        for item in cargo_lock["package"] if item.get("source")
    }
    expected.update(("npm", location, item["version"])
                    for location, item in npm_lock["packages"].items() if location)
    expected.add(("bundled-core", "mihomo", core["version"]))
    actual = []
    for item in bom.get("components", []):
        ecosystem = property_value(item, "routedeck:ecosystem")
        name = property_value(item, "routedeck:lockfile-location") if ecosystem == "npm" else item["name"]
        if not item.get("licenses") or not property_value(item, "routedeck:declared-license").strip():
            raise ReleaseError("Generated SBOM contains a dependency without a license declaration")
        actual.append((ecosystem, name, item["version"]))
    if len(actual) != len(expected) or set(actual) != expected:
        raise ReleaseError("Generated SBOM does not cover the complete locked dependency set")
    inventory = (output / "license-inventory.md").read_text(encoding="utf-8")
    if inventory != compliance_markdown(bom, inputs):
        raise ReleaseError("License inventory does not match its verified SBOM")


def prepare_assets(root, artifacts, output, tag, fetch_source=download_source,
                   generate_inventory=generate_compliance):
    version, notes = validate_version(root, tag)
    packages = collect_packages(artifacts, version)
    manifest = json.loads((root / "src-tauri/core-manifest.json").read_text(encoding="utf-8"))
    core = manifest["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", core):
        raise ReleaseError("Invalid bundled core version")
    source_url = f"https://github.com/MetaCubeX/mihomo/archive/refs/tags/v{core}.tar.gz"
    license_path = root / "third-party/Mihomo-LICENSE.txt"
    if (manifest.get("sourceUrl") != source_url or manifest.get("license") != "GPL-3.0"
            or sha256(license_path) != manifest.get("licenseSha256")):
        raise ReleaseError("Bundled source or license does not match the core manifest")
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise ReleaseError("The release staging directory must be empty")
    with tempfile.TemporaryDirectory(prefix="mihomo-compliance-") as folder:
        compliance = Path(folder)
        generate_inventory(root, compliance)
        validate_compliance(root, compliance, version)
        for name in ("sbom.cdx.json", "license-inventory.md"):
            shutil.copyfile(compliance / name, output / name)
    shutil.copyfile(root / "LICENSE", output / "RouteDeck-LICENSE.txt")
    for path in packages:
        shutil.copyfile(path, output / path.name)
    stage_updaters(root, artifacts, output, version, notes)
    shutil.copyfile(license_path, output / f"Mihomo-LICENSE-v{core}.txt")
    source_path = output / f"mihomo-v{core}-source.tar.gz"
    fetch_source(source_url, source_path)
    with tarfile.open(source_path, "r:gz") as archive:
        member = archive.getmember(f"mihomo-{core}/LICENSE")
        if not member.isfile() or member.size != license_path.stat().st_size:
            raise ReleaseError("Upstream source does not contain the matching core license")
        if archive.extractfile(member).read() != license_path.read_bytes():
            raise ReleaseError("Upstream source license differs from the bundled core license")
    assets = sorted(output.iterdir(), key=lambda path: path.name)
    checksums = output / "SHA256SUMS.txt"
    checksums.write_text("".join(f"{sha256(path)}  {path.name}\n" for path in assets), encoding="utf-8", newline="\n")
    return [*assets, checksums], notes


class GitHub:
    """Use gh's supported authentication and private-asset redirect handling."""

    def command(self, args, *, data=None, output=None):
        result = subprocess.run(["gh", *args], input=data, stdout=output or subprocess.PIPE,
                                stderr=subprocess.PIPE, check=False)
        if result.returncode:
            # Do not print request bodies, token-bearing environments or signed download URLs.
            raise ReleaseError(f"GitHub CLI request failed (exit {result.returncode}); no assets were replaced")
        return result.stdout

    def api(self, path, method="GET", data=None):
        args = ["api", path, "--method", method]
        payload = None
        if data is not None:
            args.extend(["--input", "-"])
            payload = json.dumps(data).encode("utf-8")
        return json.loads(self.command(args, data=payload))

    def pages(self, path):
        rows = []
        for page in range(1, 1001):
            batch = self.api(f"{path}?per_page=100&page={page}")
            rows.extend(batch)
            if len(batch) < 100:
                return rows
        raise ReleaseError("GitHub pagination limit reached")

    def upload(self, release_id, path):
        endpoint = f"https://uploads.github.com/repos/{REPOSITORY}/releases/{release_id}/assets?name={quote(path.name)}"
        return json.loads(self.command(["api", endpoint, "--method", "POST", "--input", str(path),
                                        "-H", "Content-Type: application/octet-stream"]))

    def digest(self, asset):
        digest = asset.get("digest") or ""
        if re.fullmatch(r"sha256:[a-f0-9]{64}", digest):
            return digest.removeprefix("sha256:")
        with tempfile.TemporaryFile() as output:
            self.command(["api", f"/repos/{REPOSITORY}/releases/assets/{int(asset['id'])}",
                          "-H", "Accept: application/octet-stream"], output=output)
            output.seek(0)
            return hashlib.file_digest(output, "sha256").hexdigest()


def verify_tag(api, tag, commit):
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ReleaseError("A full build commit SHA is required")
    obj = api.api(f"/repos/{REPOSITORY}/git/ref/tags/{quote(tag, safe='')}")["object"]
    for _ in range(10):
        if obj["type"] != "tag":
            break
        obj = api.api(f"/repos/{REPOSITORY}/git/tags/{obj['sha']}")["object"]
    if obj["type"] != "commit" or obj["sha"] != commit:
        raise ReleaseError("Remote release tag no longer matches the build commit")


def verify_asset(api, remote, path):
    if (remote.get("state") != "uploaded" or remote.get("size") != path.stat().st_size
            or api.digest(remote) != sha256(path)):
        raise ReleaseError(f"Asset content conflict or incomplete upload: {path.name}; refusing replacement")


def publish(api, tag, commit, assets, notes):
    title = release_title(tag)
    verify_tag(api, tag, commit)
    endpoint = f"/repos/{REPOSITORY}/releases"
    releases = [release for release in api.pages(endpoint) if release.get("tag_name") == tag]
    if len(releases) > 1:
        raise ReleaseError("More than one release exists for the tag")
    release = releases[0] if releases else api.api(endpoint, "POST", {
        "tag_name": tag, "target_commitish": commit, "name": title,
        "body": notes, "draft": True, "prerelease": False,
    })
    if release.get("tag_name") != tag or release.get("prerelease"):
        raise ReleaseError("Unexpected release metadata")
    release_id = int(release["id"])
    asset_endpoint = f"{endpoint}/{release_id}/assets"
    remote = api.pages(asset_endpoint)
    existing = {asset["name"]: asset for asset in remote}
    expected = {path.name for path in assets}
    if len(existing) != len(remote) or set(existing) - expected:
        raise ReleaseError("Unexpected or duplicated existing release assets; refusing to modify")
    # Check all existing names before any upload, so reruns never overwrite old builds.
    for path in assets:
        if path.name in existing:
            verify_asset(api, existing[path.name], path)
    for path in assets:
        if path.name not in existing:
            uploaded = api.upload(release_id, path)
            if uploaded.get("name") != path.name:
                raise ReleaseError("Uploaded asset name changed unexpectedly")
            verify_asset(api, uploaded, path)
    remote = api.pages(asset_endpoint)
    if len(remote) != len(assets) or {asset["name"] for asset in remote} != expected:
        raise ReleaseError("Release asset set is incomplete; leaving draft unpublished")
    by_name = {asset["name"]: asset for asset in remote}
    for path in assets:
        verify_asset(api, by_name[path.name], path)
    verify_tag(api, tag, commit)
    if release.get("draft"):
        api.api(f"{endpoint}/{release_id}", "PATCH", {
            "name": title, "body": notes,
            "draft": False, "make_latest": "legacy",
        })
    confirmed = api.api(f"{endpoint}/{release_id}")
    if confirmed.get("draft") is not False or confirmed.get("tag_name") != tag:
        raise ReleaseError("GitHub did not confirm release publication")
    if release.get("draft") and (confirmed.get("name") != title or confirmed.get("body") != notes):
        raise ReleaseError("GitHub did not confirm the reviewed release title and notes")
    print(f"Published and verified {tag}: {len(assets)} assets")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    if os.environ.get("GITHUB_REPOSITORY", REPOSITORY) != REPOSITORY:
        raise ReleaseError("Publishing is restricted to CMMUU/routedeck")
    root = Path(__file__).resolve().parent.parent
    if args.apply:
        checked_out = subprocess.run(["git", "-C", str(root), "rev-parse", "HEAD"],
                                     capture_output=True, text=True, check=False)
        if checked_out.returncode or checked_out.stdout.strip() != args.commit:
            raise ReleaseError("The checked-out source does not match the build commit")
        clean_source = subprocess.run(["git", "-C", str(root), "status", "--porcelain", "--untracked-files=all"], capture_output=True, text=True, check=False)
        if clean_source.returncode or clean_source.stdout.strip():
            raise ReleaseError("Uncommitted or untracked source changes must not be included in a tagged release")
    assets, notes = prepare_assets(root, args.artifacts, args.output, args.tag)
    if args.apply:
        publish(GitHub(), args.tag, args.commit, assets, notes)
    else:
        print(f"Prepared {len(assets)} release assets; no GitHub changes without --apply")


if __name__ == "__main__":
    try:
        main()
    except (ReleaseError, OSError, ValueError, KeyError, tarfile.TarError) as error:
        raise SystemExit(f"Release failed: {error}")
