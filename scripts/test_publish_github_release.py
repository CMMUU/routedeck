import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from generate_compliance import (INPUT_PATHS, cargo_components, core_component,
                                 markdown as compliance_markdown, npm_components)
from publish_github_release import (ReleaseError, collect_packages, package_names,
                                    prepare_assets, publish, release_title, sha256, validate_version,
                                    validate_compliance)


COMMIT = "a" * 40
TAG = "v0.6.0"
ENDPOINT = "/repos/CMMUU/serylane/releases"


class FakeGitHub:
    def __init__(self):
        self.release = None
        self.assets = []
        self.writes = []
        self.uploads = []
        self.fail_upload = False
        self.commit = COMMIT
        self.move_after_upload = False

    def api(self, path, method="GET", data=None):
        if "/git/ref/tags/" in path:
            return {"object": {"type": "tag", "sha": "b" * 40}}
        if "/git/tags/" in path:
            return {"object": {"type": "commit", "sha": self.commit}}
        if method == "GET" and path == ENDPOINT + "/1":
            return self.release.copy()
        self.writes.append((method, data))
        if method == "POST" and path == ENDPOINT:
            self.release = {**data, "id": 1}
        elif method == "PATCH" and path == ENDPOINT + "/1":
            self.release.update(data)
        else:
            raise AssertionError((path, method))
        return self.release.copy()

    def pages(self, path):
        if path == ENDPOINT:
            return [self.release.copy()] if self.release else []
        if path == ENDPOINT + "/1/assets":
            return [asset.copy() for asset in self.assets]
        raise AssertionError(path)

    def upload(self, release_id, path):
        if self.fail_upload:
            raise ReleaseError("Simulated upload failure")
        asset = {"name": path.name, "size": path.stat().st_size, "state": "uploaded",
                 "digest": "sha256:" + sha256(path), "id": len(self.assets) + 1}
        self.assets.append(asset)
        self.uploads.append(path.name)
        if self.move_after_upload:
            self.commit = "c" * 40
        return asset.copy()

    def digest(self, asset):
        return asset["digest"].removeprefix("sha256:")


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.root = self.base / "repo"
        self.artifacts = self.base / "artifacts"
        self.output = self.base / "release"
        for folder in ("src-tauri", "third-party", "docs"):
            (self.root / folder).mkdir(parents=True)
        self.project = {"name": "routedeck", "version": "0.6.0", "license": "GPL-3.0-only"}
        for path in ("package.json", "src-tauri/tauri.conf.json"):
            (self.root / path).write_text(json.dumps(self.project), encoding="utf-8")
        self.npm_lock = {"version": "0.6.0", "packages": {"": self.project, "node_modules/fixture": {
            "version": "1.0.0", "license": "MIT", "integrity": "sha256-" + "A" * 43 + "=",
            "resolved": "https://registry.npmjs.org/fixture/-/fixture-1.0.0.tgz",
        }}}
        (self.root / "package-lock.json").write_text(json.dumps(self.npm_lock), encoding="utf-8")
        (self.root / "src-tauri/Cargo.toml").write_text(
            '[package]\nname = "routedeck"\nversion = "0.6.0"\nlicense = "GPL-3.0-only"\n', encoding="utf-8")
        (self.root / "src-tauri/Cargo.lock").write_text(
            'version = 4\n[[package]]\nname = "fixture"\nversion = "1.0.0"\n'
            'source = "registry+https://github.com/rust-lang/crates.io-index"\n'
            'checksum = "' + "0" * 64 + '"\n[[package]]\nname = "routedeck"\nversion = "0.6.0"\n', encoding="utf-8")
        (self.root / "docs/发布说明-v0.6.0.md").write_text("Reviewed notes\n", encoding="utf-8")
        self.license = b"GPL source license fixture\n"
        (self.root / "third-party/Mihomo-LICENSE.txt").write_bytes(self.license)
        (self.root / "LICENSE").write_bytes(self.license)
        self.manifest = {
            "version": "1.19.30", "license": "GPL-3.0",
            "licenseSha256": hashlib.sha256(self.license).hexdigest(),
            "sourceUrl": "https://github.com/MetaCubeX/mihomo/archive/refs/tags/v1.19.30.tar.gz",
            "repository": "https://github.com/MetaCubeX/mihomo",
            "licenseUrl": "https://raw.githubusercontent.com/MetaCubeX/mihomo/v1.19.30/LICENSE",
            "releaseBaseUrl": "https://github.com/MetaCubeX/mihomo/releases/download/v1.19.30",
            "targets": {},
        }
        self.write_manifest()
        for platform, names in package_names("0.6.0").items():
            folder = self.artifacts / platform / "bundle"
            folder.mkdir(parents=True)
            for name in names:
                (folder / name).write_bytes(f"Built package: {name}".encode())

    def write_manifest(self):
        (self.root / "src-tauri/core-manifest.json").write_text(json.dumps(self.manifest), encoding="utf-8")

    def fetch_source(self, url, destination):
        self.assertEqual(url, self.manifest["sourceUrl"])
        with tarfile.open(destination, "w:gz") as archive:
            member = tarfile.TarInfo("mihomo-1.19.30/LICENSE")
            member.size = len(self.license)
            archive.addfile(member, io.BytesIO(self.license))

    def prepare(self):
        return prepare_assets(self.root, self.artifacts, self.output, TAG, self.fetch_source, self.generate_inventory)

    def generate_inventory(self, root, output):
        """Offline metadata fixture: exercise real inventory serialization without Cargo/network."""
        import tomllib
        lock = tomllib.loads((root / "src-tauri/Cargo.lock").read_text(encoding="utf-8"))
        metadata = {"packages": [{**lock["package"][0], "license": "MIT"}]}
        components = cargo_components(lock, metadata) + npm_components(self.npm_lock) + [core_component(self.manifest)]
        inputs = {path: sha256(root / path) for path in INPUT_PATHS}
        bom = {"bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
               "metadata": {"component": {**self.project, "licenses": [{"expression": "GPL-3.0-only"}]},
                            "properties": [{"name": f"routedeck:input-sha256:{path}", "value": value}
                                           for path, value in inputs.items()]},
               "components": components}
        (output / "sbom.cdx.json").write_text(json.dumps(bom), encoding="utf-8")
        (output / "license-inventory.md").write_text(compliance_markdown(bom, inputs), encoding="utf-8", newline="\n")

    def test_complete_platform_set_produces_eighteen_assets_and_exact_checksums(self):
        assets, notes = self.prepare()
        self.assertEqual(len(assets), 18)
        self.assertEqual((self.output / "RouteDeck-LICENSE.txt").read_bytes(), self.license)
        self.assertTrue({"sbom.cdx.json", "license-inventory.md"}.issubset({path.name for path in assets}))
        self.assertEqual(notes, "Reviewed notes\n")
        expected = {path.name: sha256(path) for path in assets if path.name != "SHA256SUMS.txt"}
        actual = {line.split("  ", 1)[1]: line.split("  ", 1)[0]
                  for line in (self.output / "SHA256SUMS.txt").read_text().splitlines()}
        self.assertEqual(actual, expected)
        self.assertNotIn(b"\r", (self.output / "SHA256SUMS.txt").read_bytes())

    def test_bundle_names_preserve_product_name_case_for_every_platform(self):
        self.assertEqual(package_names("0.6.0"), {
            "macos-aarch64": ["RouteDeck_0.6.0_aarch64.dmg"],
            "macos-x64": ["RouteDeck_0.6.0_x64.dmg"],
            "windows-x64": ["RouteDeck_0.6.0_x64-setup.exe", "RouteDeck_0.6.0_x64_en-US.msi"],
            "windows-arm64": ["RouteDeck_0.6.0_arm64-setup.exe", "RouteDeck_0.6.0_arm64_en-US.msi"],
            "linux-x64": ["RouteDeck_0.6.0_amd64.AppImage", "RouteDeck_0.6.0_amd64.deb",
                          "RouteDeck-0.6.0-1.x86_64.rpm"],
            "linux-arm64": ["RouteDeck_0.6.0_aarch64.AppImage", "RouteDeck_0.6.0_arm64.deb",
                            "RouteDeck-0.6.0-1.aarch64.rpm"],
        })

    def test_cargo_inventory_only_excludes_the_renamed_application(self):
        self.assertEqual(cargo_components({"package": [{"name": "routedeck", "version": "0.6.0"}]},
                                          {"packages": []}), [])
        for name in ("mihomo-codex", "RouteDeck", "unreviewed-local-package"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "Unreviewed local package"):
                cargo_components({"package": [{"name": name, "version": "0.6.0"}]}, {"packages": []})

    def test_current_brand_requires_serylane_package_and_preserves_historical_validation(self):
        self.assertEqual(validate_version(self.root, TAG)[0], "0.6.0")
        for relative in ("package.json", "package-lock.json", "src-tauri/tauri.conf.json",
                         "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"):
            path = self.root / relative
            path.write_text(path.read_text(encoding="utf-8").replace("0.6.0", "0.7.7").replace("routedeck", "serylane"), encoding="utf-8")
        config_path = self.root / "src-tauri/tauri.conf.json"
        config = json.loads(config_path.read_text(encoding="utf-8"))
        config["bundle"] = {"createUpdaterArtifacts": True}
        config_path.write_text(json.dumps(config), encoding="utf-8")
        (self.root / "docs/发布说明-v0.7.7.md").write_text("Reviewed Serylane notes", encoding="utf-8")
        self.assertEqual(validate_version(self.root, "v0.7.7")[0], "0.7.7")
        self.assertEqual(cargo_components({"package": [{"name": "serylane", "version": "0.7.7"}]}, {"packages": []}), [])
        with self.assertRaisesRegex(ValueError, "Unreviewed local package"):
            cargo_components({"package": [{"name": "routedeck", "version": "0.7.7"}]}, {"packages": []})
        lock_path = self.root / "src-tauri/Cargo.lock"
        lock_path.write_text(lock_path.read_text(encoding="utf-8").replace("serylane", "routedeck"), encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "exactly one application"):
            validate_version(self.root, "v0.7.7")

    def test_legacy_named_sbom_cannot_be_used_for_the_renamed_release(self):
        self.prepare()
        path = self.output / "sbom.cdx.json"
        bom = json.loads(path.read_text(encoding="utf-8"))
        bom["metadata"]["component"]["name"] = "mihomo-codex"
        path.write_text(json.dumps(bom), encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "application metadata"):
            validate_compliance(self.root, self.output, "0.6.0")

    def test_missing_one_platform_package_prevents_staging(self):
        next((self.artifacts / "windows-x64").rglob("*.msi")).unlink()
        with self.assertRaisesRegex(ReleaseError, "Missing"):
            self.prepare()
        self.assertFalse(self.output.exists())

    def test_duplicate_filename_or_wrong_version_package_is_rejected(self):
        sample = next((self.artifacts / "macos-x64").rglob("*.dmg"))
        (sample.parent.parent / sample.name).write_bytes(sample.read_bytes())
        with self.assertRaisesRegex(ReleaseError, "duplicated"):
            collect_packages(self.artifacts, "0.6.0")

    def test_tag_version_mismatch_is_rejected(self):
        with self.assertRaisesRegex(ReleaseError, "versions"):
            validate_version(self.root, "v0.7.0")

    def test_missing_reviewed_notes_is_rejected(self):
        (self.root / "docs/发布说明-v0.6.0.md").unlink()
        with self.assertRaisesRegex(ReleaseError, "Reviewed release notes"):
            self.prepare()

    def test_core_license_mismatch_is_rejected(self):
        (self.root / "third-party/Mihomo-LICENSE.txt").write_bytes(b"Changed license")
        with self.assertRaisesRegex(ReleaseError, "manifest"):
            self.prepare()

    def test_source_url_cannot_be_redirected_to_unrelated_origin_in_manifest(self):
        self.manifest["sourceUrl"] = "https://example.com/untrusted.tar.gz"
        self.write_manifest()
        with self.assertRaisesRegex(ReleaseError, "manifest"):
            self.prepare()

    def test_downloaded_source_must_contain_matching_license(self):
        self.license = b"GPL source license WRONG!!\n"
        with self.assertRaisesRegex(ReleaseError, "license"):
            self.prepare()

    def test_success_creates_draft_then_publishes_and_rerun_writes_nothing(self):
        assets, notes = self.prepare()
        api = FakeGitHub()
        publish(api, TAG, COMMIT, assets, notes)
        self.assertEqual(len(api.uploads), 18)
        self.assertTrue(api.writes[0][1]["draft"])
        self.assertFalse(api.writes[-1][1]["draft"])
        self.assertFalse(api.release["draft"])
        api.writes.clear()
        api.uploads.clear()
        api.release.update({"name": "Maintainer-edited title", "body": "Maintainer-edited published notes"})
        publish(api, TAG, COMMIT, assets, notes)
        self.assertEqual(api.writes, [])
        self.assertEqual(api.uploads, [])
        self.assertEqual(api.release["name"], "Maintainer-edited title")
        self.assertEqual(api.release["body"], "Maintainer-edited published notes")

    def test_existing_placeholder_draft_is_published_with_reviewed_title_and_notes(self):
        assets, notes = self.prepare()
        api = FakeGitHub()
        api.release = {"id": 1, "tag_name": TAG, "draft": True, "prerelease": False,
                       "name": "Work in progress", "body": "TODO: release notes"}
        publish(api, TAG, COMMIT, assets, notes)
        self.assertEqual(len(api.writes), 1)
        method, payload = api.writes[0]
        self.assertEqual(method, "PATCH")
        self.assertFalse(payload["draft"])
        self.assertEqual(payload["name"], f"RouteDeck {TAG}")
        self.assertEqual(payload["body"], notes)
        self.assertEqual(api.release["name"], f"RouteDeck {TAG}")
        self.assertEqual(api.release["body"], notes)

    def test_brand_transition_preserves_historical_names_and_renames_new_packages(self):
        self.assertEqual(release_title("v0.7.3"), "RouteDeck v0.7.3")
        for version in ("0.7.4", "0.8.0", "0.10.0", "1.0.0"):
            self.assertEqual(release_title(f"v{version}"), f"Serylane v{version}")
            self.assertEqual(package_names(version), {
                platform: [name.replace("0.7.3", version).replace("RouteDeck", "Serylane" if tuple(map(int, version.split('.'))) >= (0, 7, 7) else "RouteDeck") for name in names]
                for platform, names in package_names("0.7.3").items()
            })
        for invalid in ("v0.7.04", "0.7.4", "v0.7.4-beta.1", "latest"):
            with self.assertRaisesRegex(ReleaseError, "stable version tag"):
                release_title(invalid)

    def test_new_release_and_placeholder_draft_use_serylane_title_without_rewriting_notes(self):
        asset = self.base / "RouteDeck_0.7.4_x64-setup.exe"
        asset.write_bytes(b"synthetic updater fixture; never executed")
        notes = "# Serylane v0.7.4\n\nInstallation identity remains RouteDeck.\n"
        for existing in (False, True):
            with self.subTest(existing_draft=existing):
                api = FakeGitHub()
                if existing:
                    api.release = {"id": 1, "tag_name": "v0.7.4", "draft": True,
                                   "prerelease": False, "name": "RouteDeck draft", "body": "TODO"}
                publish(api, "v0.7.4", COMMIT, [asset], notes)
                self.assertEqual(api.release["name"], "Serylane v0.7.4")
                self.assertEqual(api.release["body"], notes)
                self.assertEqual(api.uploads, ["RouteDeck_0.7.4_x64-setup.exe"])
                self.assertTrue(all(data["name"] == "Serylane v0.7.4" for _, data in api.writes))

    def test_published_titles_and_notes_are_not_rebranded_even_after_transition(self):
        asset = self.base / "retained-package.zip"
        asset.write_bytes(b"historical package fixture")
        for tag in ("v0.7.3", "v0.7.4"):
            with self.subTest(tag=tag):
                api = FakeGitHub()
                original = {"id": 1, "tag_name": tag, "draft": False, "prerelease": False,
                            "name": f"RouteDeck {tag}", "body": "Original RouteDeck notes"}
                api.release = original.copy()
                api.upload(1, asset)
                api.uploads.clear()
                publish(api, tag, COMMIT, [asset], "Serylane replacement notes must not be written")
                self.assertEqual(api.release, original)
                self.assertEqual(api.writes, [])
                self.assertEqual(api.uploads, [])

    def test_upload_failure_leaves_draft_and_retry_resumes(self):
        assets, notes = self.prepare()
        api = FakeGitHub()
        api.fail_upload = True
        with self.assertRaisesRegex(ReleaseError, "upload failure"):
            publish(api, TAG, COMMIT, assets, notes)
        self.assertTrue(api.release["draft"])
        api.fail_upload = False
        api.upload(1, assets[0])
        publish(api, TAG, COMMIT, assets, notes)
        self.assertEqual(len(api.uploads), 18)
        self.assertFalse(api.release["draft"])

    def test_existing_content_conflict_blocks_every_upload_and_metadata_change(self):
        assets, notes = self.prepare()
        api = FakeGitHub()
        api.release = {"id": 1, "tag_name": TAG, "draft": True, "prerelease": False}
        api.upload(1, assets[-1])
        api.assets[-1]["digest"] = "sha256:" + "0" * 64
        api.uploads.clear()
        with self.assertRaisesRegex(ReleaseError, "conflict"):
            publish(api, TAG, COMMIT, assets, notes)
        self.assertEqual(api.uploads, [])
        self.assertEqual(api.writes, [])

    def test_remote_tag_mismatch_blocks_release_creation(self):
        assets, notes = self.prepare()
        api = FakeGitHub()
        api.commit = "c" * 40
        with self.assertRaisesRegex(ReleaseError, "build commit"):
            publish(api, TAG, COMMIT, assets, notes)
        self.assertEqual(api.writes, [])

    def test_tag_moved_during_upload_leaves_draft_unpublished(self):
        assets, notes = self.prepare()
        api = FakeGitHub()
        api.move_after_upload = True
        with self.assertRaisesRegex(ReleaseError, "build commit"):
            publish(api, TAG, COMMIT, assets, notes)
        self.assertTrue(api.release["draft"])
        self.assertEqual(len(api.writes), 1)

    def test_stale_sbom_cannot_describe_different_lockfile_contents(self):
        self.prepare()
        lock = self.root / "src-tauri/Cargo.lock"
        lock.write_text(lock.read_text(encoding="utf-8") + "\n# changed after generation\n", encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "input checksums"):
            validate_compliance(self.root, self.output, "0.6.0")

    def test_sbom_cannot_omit_locked_dependency_even_with_current_input_hashes(self):
        self.prepare()
        path = self.output / "sbom.cdx.json"
        bom = json.loads(path.read_text(encoding="utf-8"))
        bom["components"].pop(0)
        path.write_text(json.dumps(bom), encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "complete locked dependency set"):
            validate_compliance(self.root, self.output, "0.6.0")

    def test_sbom_duplicate_input_digest_is_rejected(self):
        self.prepare()
        path = self.output / "sbom.cdx.json"
        bom = json.loads(path.read_text(encoding="utf-8"))
        bom["metadata"]["properties"].append(bom["metadata"]["properties"][0])
        path.write_text(json.dumps(bom), encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "input checksums"):
            validate_compliance(self.root, self.output, "0.6.0")

    def test_inventory_must_match_the_verified_sbom(self):
        self.prepare()
        (self.output / "license-inventory.md").write_text("Different inventory\n", encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "inventory does not match"):
            validate_compliance(self.root, self.output, "0.6.0")

    def test_missing_generated_compliance_files_prevent_asset_staging(self):
        with self.assertRaisesRegex(ReleaseError, "Missing generated compliance asset"):
            prepare_assets(self.root, self.artifacts, self.output, TAG, self.fetch_source,
                           lambda root, output: None)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_project_gpl_metadata_and_complete_license_text_are_required(self):
        (self.root / "LICENSE").write_bytes(b"GPL-3.0-only")
        with self.assertRaisesRegex(ReleaseError, "complete pinned text"):
            self.prepare()

    def test_mismatched_npm_license_is_rejected(self):
        self.npm_lock["packages"][""]["license"] = "MIT"
        (self.root / "package-lock.json").write_text(json.dumps(self.npm_lock), encoding="utf-8")
        with self.assertRaisesRegex(ReleaseError, "license metadata"):
            self.prepare()

    def test_existing_fifteen_asset_release_is_never_replaced_or_expanded(self):
        assets, notes = self.prepare()
        legacy = [path for path in assets if path.name not in {
            "RouteDeck-LICENSE.txt", "sbom.cdx.json", "license-inventory.md", "SHA256SUMS.txt",
        }]
        legacy_dir = self.base / "legacy"
        legacy_dir.mkdir()
        checksums = legacy_dir / "SHA256SUMS.txt"
        checksums.write_text("".join(f"{sha256(path)}  {path.name}\n" for path in legacy), encoding="utf-8")
        api = FakeGitHub()
        publish(api, TAG, COMMIT, [*legacy, checksums], notes)
        self.assertEqual(len(api.assets), 15)
        api.uploads.clear()
        api.writes.clear()
        with self.assertRaisesRegex(ReleaseError, "conflict"):
            publish(api, TAG, COMMIT, assets, notes)
        self.assertEqual(len(api.assets), 15)
        self.assertEqual(api.uploads, [])
        self.assertEqual(api.writes, [])


if __name__ == "__main__":
    unittest.main()
