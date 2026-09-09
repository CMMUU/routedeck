#!/usr/bin/env python3
"""Generate a deterministic, all-platform lockfile SBOM and license inventory.

This inventory describes locked packages, not the contents of a specific binary.
It needs Python 3.11+ and locked Cargo metadata (Cargo may download crate sources).
"""

from __future__ import annotations

import argparse
import base64
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import quote, urlsplit


ROOT = Path(__file__).resolve().parents[1]
INPUT_PATHS = (
    "package.json", "package-lock.json", "src-tauri/Cargo.toml",
    "src-tauri/Cargo.lock", "src-tauri/core-manifest.json", "LICENSE",
    "third-party/Mihomo-LICENSE.txt",
)


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8-sig"))


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def public_url(value: str | None) -> str | None:
    """Never copy authenticated or local registry addresses into public output."""
    if not value:
        return None
    parsed = urlsplit(value)
    if parsed.scheme not in {"https", "http"}:
        return None
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("A dependency URL contains authentication or query data")
    if not parsed.hostname or parsed.hostname in {"localhost", "127.0.0.1", "::1"}:
        raise ValueError("A dependency URL is not a public source address")
    return value


def declared_license(value: str | None, name: str) -> list[dict]:
    if not value or value.strip().upper() in {"UNLICENSED", "UNKNOWN", "NONE"}:
        raise ValueError(f"Missing reviewable license declaration for {name}")
    # Older Cargo manifests used '/' to mean alternative licensing. Preserve the
    # original declaration in a property and render its SPDX equivalent here.
    expression = re.sub(r"\s*/\s*", " OR ", value.strip())
    return [{"expression": expression}]


def npm_hashes(integrity: str | None) -> list[dict]:
    values = []
    for token in (integrity or "").split():
        algorithm, encoded = token.split("-", 1)
        if algorithm not in {"sha1", "sha256", "sha384", "sha512"}:
            raise ValueError(f"Unsupported npm integrity algorithm: {algorithm}")
        raw = base64.b64decode(encoded, validate=True)
        expected_length = {"sha1": 20, "sha256": 32, "sha384": 48, "sha512": 64}[algorithm]
        if len(raw) != expected_length:
            raise ValueError("Incorrect npm integrity hash length")
        values.append({"alg": "SHA-" + algorithm[3:], "content": raw.hex()})
    if not values:
        raise ValueError("Missing npm package integrity")
    return values


def cargo_components(lock: dict, metadata: dict) -> list[dict]:
    packages = {
        (item["name"], item["version"], item.get("source")): item
        for item in metadata["packages"]
    }
    components = []
    for locked in lock["package"]:
        if not locked.get("source"):
            if locked["name"] != ("serylane" if tuple(map(int, locked["version"].split("."))) >= (0, 7, 7) else "routedeck"):
                raise ValueError(f"Unreviewed local package: {locked['name']}")
            continue
        if locked["source"] != "registry+https://github.com/rust-lang/crates.io-index":
            raise ValueError(f"Unreviewed Cargo package source: {locked['name']}")
        identity = (locked["name"], locked["version"], locked["source"])
        package = packages.get(identity)
        if package is None:
            raise ValueError(f"Cargo metadata does not cover locked package {identity[:2]}")
        name, version = identity[:2]
        purl = f"pkg:cargo/{quote(name)}@{quote(version)}"
        refs = [{"type": "distribution", "url": f"https://crates.io/api/v1/crates/{name}/{version}/download"}]
        repository = public_url(package.get("repository"))
        if repository:
            refs.append({"type": "vcs", "url": repository})
        checksum = locked.get("checksum", "")
        if not re.fullmatch(r"[0-9a-f]{64}", checksum):
            raise ValueError(f"Missing archive checksum for {name}@{version}")
        components.append({
            "type": "library", "bom-ref": purl, "name": name, "version": version,
            "purl": purl, "licenses": declared_license(package.get("license"), purl),
            "hashes": [{"alg": "SHA-256", "content": checksum}],
            "externalReferences": refs,
            "properties": [
                {"name": "routedeck:ecosystem", "value": "cargo"},
                {"name": "routedeck:declared-license", "value": package["license"]},
                {"name": "routedeck:inventory-scope", "value": "all locked platforms and dependency kinds"},
            ],
        })
    return components


def npm_components(lock: dict) -> list[dict]:
    components = []
    for location, package in sorted(lock["packages"].items()):
        if not location:
            continue
        name = package.get("name") or location.rsplit("node_modules/", 1)[1]
        version = package["version"]
        purl = f"pkg:npm/{quote(name, safe='/')}@{quote(version)}"
        source = public_url(package.get("resolved"))
        if not source:
            raise ValueError(f"Missing public package source for {name}@{version}")
        components.append({
            "type": "library", "bom-ref": f"npm:{location}@{version}",
            "name": name, "version": version, "purl": purl,
            "licenses": declared_license(package.get("license"), purl),
            "hashes": npm_hashes(package.get("integrity")),
            "externalReferences": [{"type": "distribution", "url": source}],
            "properties": [
                {"name": "routedeck:ecosystem", "value": "npm"},
                {"name": "routedeck:declared-license", "value": package["license"]},
                {"name": "routedeck:lockfile-location", "value": location},
                {"name": "routedeck:development", "value": str(package.get("dev", False)).lower()},
                {"name": "routedeck:optional", "value": str(package.get("optional", False)).lower()},
                {"name": "routedeck:os", "value": ",".join(package.get("os", ["any"]))},
                {"name": "routedeck:cpu", "value": ",".join(package.get("cpu", ["any"]))},
            ],
        })
    return components


def core_component(manifest: dict) -> dict:
    refs = [
        {"type": "vcs", "url": public_url(manifest["repository"])},
        {"type": "distribution", "url": public_url(manifest["sourceUrl"])},
        {"type": "license", "url": public_url(manifest["licenseUrl"]),
         "hashes": [{"alg": "SHA-256", "content": manifest["licenseSha256"]}]},
    ]
    for target in sorted(manifest["targets"]):
        asset = manifest["targets"][target]
        refs.append({
            "type": "distribution",
            "url": public_url(f"{manifest['releaseBaseUrl']}/{asset['asset']}"),
            "comment": target,
            "hashes": [{"alg": "SHA-256", "content": asset["sha256"]}],
        })
    return {
        "type": "application", "bom-ref": "bundled:mihomo", "name": "mihomo",
        "version": manifest["version"],
        "licenses": [{"license": {"name": manifest["license"]}}],
        "externalReferences": refs,
        "properties": [
            {"name": "routedeck:ecosystem", "value": "bundled-core"},
            {"name": "routedeck:declared-license", "value": manifest["license"]},
            {"name": "routedeck:inventory-scope", "value": "pinned upstream core; internal Go dependency graph is outside this lockfile inventory"},
        ],
    }


def property_value(component: dict, name: str) -> str:
    for item in component["properties"]:
        if item["name"] == name:
            return item["value"]
    raise ValueError(f"Missing SBOM component property: {name}")


def markdown(bom: dict, inputs: dict[str, str]) -> str:
    version = bom["metadata"]["component"]["version"]
    components = bom["components"]
    groups = Counter(property_value(item, "routedeck:ecosystem") for item in components)
    lines = [
        f"# {'Serylane' if tuple(map(int, version.split('.'))) >= (0, 7, 7) else 'RouteDeck'} v{version} license inventory", "",
        "Generated by `scripts/generate_compliance.py`; do not edit package rows by hand.", "",
        "Application license: **GPL-3.0-only**. The source was opened on 2026-09-04.", "",
        "This is an all-platform lockfile inventory, including development, build and",
        "optional dependencies. It is not a per-installer binary composition report.",
        "System libraries, platform WebViews and the bundled Mihomo core's internal Go",
        "dependency graph are outside this scope. The matching upstream core source",
        "archive supplies its own source, dependency declarations and license notices.", "",
        "License expressions below are upstream declarations, not a replacement for",
        "upstream notices. Historical Cargo `/` alternatives are normalized to `OR` in",
        "the SBOM; this table preserves the original declaration. A declaration with",
        "`OR` offers alternatives; `AND` requires the applicable combined terms.", "",
        f"Coverage: **{groups['cargo']} Rust packages**, **{groups['npm']} npm lockfile entries**,",
        f"and **{groups['bundled-core']} pinned Mihomo core**. Archive hashes, npm installation",
        "paths, development/optional flags and platform constraints are recorded in",
        "[sbom.cdx.json](sbom.cdx.json). No package license declaration is missing.", "",
        "## Input checksums", "", "| Repository input | SHA-256 |", "| --- | --- |",
    ]
    lines.extend(f"| `{path}` | `{checksum}` |" for path, checksum in inputs.items())
    for ecosystem, heading in [("bundled-core", "Bundled Mihomo core"), ("cargo", "Rust / Cargo"), ("npm", "JavaScript / npm")]:
        lines.extend(["", f"## {heading}", "", "| Package | Version | Declared license | Exact source archive |", "| --- | --- | --- | --- |"])
        for item in components:
            if property_value(item, "routedeck:ecosystem") != ecosystem:
                continue
            license_text = property_value(item, "routedeck:declared-license").replace("|", "\\|")
            source = next(ref["url"] for ref in item["externalReferences"] if ref["type"] == "distribution")
            lines.append(f"| `{item['name']}` | `{item['version']}` | {license_text} | [source]({source}) |")
    return "\n".join(lines) + "\n"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo-metadata", type=Path, help="Use existing cargo metadata --locked --format-version 1 JSON")
    parser.add_argument("--output-dir", type=Path, help="Default: docs/compliance/v<package version>")
    args = parser.parse_args()
    package = read_json(ROOT / "package.json")
    npm_lock = read_json(ROOT / "package-lock.json")
    cargo_manifest = tomllib.loads((ROOT / "src-tauri/Cargo.toml").read_text(encoding="utf-8"))
    cargo_lock = tomllib.loads((ROOT / "src-tauri/Cargo.lock").read_text(encoding="utf-8"))
    manifest = read_json(ROOT / "src-tauri/core-manifest.json")
    for project in (package, npm_lock["packages"][""], cargo_manifest["package"]):
        if project["version"] != package["version"] or project.get("license") != "GPL-3.0-only":
            raise ValueError("Application versions and GPL-3.0-only metadata must agree")
    if digest(ROOT / "third-party/Mihomo-LICENSE.txt") != manifest["licenseSha256"]:
        raise ValueError("Bundled core license does not match its pinned checksum")
    if args.cargo_metadata:
        metadata = read_json(args.cargo_metadata)
    else:
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1", "--manifest-path", str(ROOT / "src-tauri/Cargo.toml")],
            check=True, stdout=subprocess.PIPE, encoding="utf-8",
        )
        metadata = json.loads(result.stdout)
    components = cargo_components(cargo_lock, metadata) + npm_components(npm_lock) + [core_component(manifest)]
    components.sort(key=lambda item: (property_value(item, "routedeck:ecosystem"), item["name"], item["version"], item["bom-ref"]))
    if len({item["bom-ref"] for item in components}) != len(components):
        raise ValueError("Duplicate SBOM component identity")
    inputs = {path: digest(ROOT / path) for path in INPUT_PATHS}
    bom = {
        "bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
        "metadata": {
            "component": {
                "type": "application", "bom-ref": f"application:{package['name']}",
                "name": package["name"], "version": package["version"],
                "licenses": [{"expression": package["license"]}],
                "externalReferences": [
                    {"type": "vcs", "url": "https://github.com/CMMUU/serylane"},
                    {"type": "vcs", "url": "https://gitee.com/cmmuu/serylane"},
                ],
            },
            "properties": [
                {"name": "routedeck:inventory-scope", "value": "locked Rust/npm packages across platforms including optional/build/dev packages, plus pinned Mihomo core; excludes system libraries, platform WebViews and upstream core's internal Go graph"},
                *[{"name": f"routedeck:input-sha256:{path}", "value": checksum} for path, checksum in inputs.items()],
            ],
        },
        "components": components,
    }
    output = args.output_dir or ROOT / "docs" / "compliance" / f"v{package['version']}"
    output.mkdir(parents=True, exist_ok=True)
    (output / "sbom.cdx.json").write_text(json.dumps(bom, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
    (output / "license-inventory.md").write_text(markdown(bom, inputs), encoding="utf-8", newline="\n")
    print(f"Generated {len(components)} dependency entries in {output}")


if __name__ == "__main__":
    main()
