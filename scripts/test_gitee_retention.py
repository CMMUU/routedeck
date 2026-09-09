"""Offline release-retention tests. No accounts, credentials or remote writes."""
from copy import deepcopy
import hashlib
from io import BytesIO
from pathlib import Path
import re
import tempfile
import unittest
from unittest.mock import patch

import sync_gitee as sync


def repo_metadata(owner):
    repo = "serylane"
    return {"full_name": f"{owner}/{repo}", "path": repo, "private": False,
            "owner": {"login": owner}, "html_url": f"https://gitee.com/{owner}/{repo}"}


class Source:
    def __init__(self, events):
        self.events, self.releases, self.assets, self.contents = events, [], {}, {}

    def add(self, tag, data=b"good", **flags):
        release_id = len(self.releases) + 1
        release = {"id": release_id, "tag_name": tag, "draft": False, "prerelease": False,
                   "name": tag, "body": "notes", **flags}
        self.releases.append(release)
        asset_id = release_id * 10
        self.contents[asset_id] = data
        self.assets[release_id] = [{"id": asset_id, "name": "package.zip", "size": len(data),
                                   "state": "uploaded", "digest": "sha256:" + hashlib.sha256(data).hexdigest(),
                                   "url": f"https://api.github.com/repos/CMMUU/serylane/releases/assets/{asset_id}"}]
        return release

    def request(self, path):
        assert path == "/repos/CMMUU/serylane"
        return repo_metadata("CMMUU")

    def pages(self, path):
        if path.endswith("/releases"):
            return deepcopy(self.releases)
        match = re.fullmatch(r"/repos/CMMUU/serylane/releases/(\d+)/assets", path)
        assert match, path
        return deepcopy(self.assets[int(match[1])])

    def download(self, path, destination, expected_size, expected_sha):
        asset_id = int(path.rsplit("/", 1)[1])
        self.events.append(("source-download", asset_id))
        data = self.contents[asset_id]
        digest = hashlib.sha256(data).hexdigest()
        if len(data) != expected_size or (expected_sha is not None and digest != expected_sha):
            raise sync.SyncError("Source digest mismatch")
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        return digest


class Destination:
    def __init__(self, events):
        self.events, self.releases, self.assets, self.contents = events, {}, {}, {}
        self.next_id, self.delete_error, self.remove_on_error, self.upload_error = 500, False, False, False

    def add(self, source, gh):
        release_id = source["id"] + 100
        self.releases[release_id] = {**deepcopy(source), "id": release_id}
        self.assets[release_id] = []
        for asset in gh.assets[source["id"]]:
            asset_id = asset["id"] + 100
            self.assets[release_id].append({"id": asset_id, "name": asset["name"], "size": asset["size"]})
            self.contents[asset_id] = gh.contents[asset["id"]]

    def pages(self, path):
        if path.endswith("/releases"):
            return deepcopy(list(self.releases.values()))
        match = re.fullmatch(r"/repos/cmmuu/serylane/releases/(\d+)/attach_files", path)
        assert match, path
        return deepcopy(self.assets[int(match[1])])

    def request(self, path, method="GET", data=None):
        if path == "/user":
            return {"login": "cmmuu"}
        if path == "/repos/cmmuu/serylane":
            return repo_metadata("cmmuu")
        if method == "POST" and path.endswith("/releases"):
            self.next_id += 1
            self.releases[self.next_id] = {**data, "id": self.next_id, "prerelease": data["prerelease"] == "true"}
            self.assets[self.next_id] = []
            self.events.append(("create", data["tag_name"]))
            return deepcopy(self.releases[self.next_id])
        match = re.fullmatch(r"/repos/cmmuu/serylane/releases/(\d+)(?:/attach_files/(\d+))?", path)
        assert match, (path, method)
        release_id = int(match[1])
        if method == "GET":
            return deepcopy(self.releases[release_id])
        if method == "PATCH":
            self.releases[release_id].update(data)
            self.releases[release_id]["prerelease"] = data["prerelease"] == "true"
            return deepcopy(self.releases[release_id])
        assert method == "DELETE" and match[2], (method, path)
        asset_id = int(match[2])
        self.events.append(("delete", release_id, asset_id))
        if not self.delete_error or self.remove_on_error:
            self.assets[release_id] = [asset for asset in self.assets[release_id] if asset["id"] != asset_id]
        if self.delete_error:
            raise sync.SyncError("Delete outcome uncertain")

    def download(self, path, destination, expected_size, expected_sha):
        asset_id = int(path.split("/")[-2])
        self.events.append(("destination-download", asset_id))
        data = self.contents[asset_id]
        if len(data) != expected_size or hashlib.sha256(data).hexdigest() != expected_sha:
            raise sync.SyncError("Destination digest mismatch")
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        return expected_sha

    def upload(self, path, file):
        if self.upload_error:
            raise sync.SyncError("Upload failed")
        release_id = int(path.split("/")[-2])
        self.next_id += 1
        self.assets[release_id].append({"id": self.next_id, "name": file.name, "size": file.stat().st_size})
        self.contents[self.next_id] = file.read_bytes()
        self.events.append(("upload", release_id))


class RetentionTests(unittest.TestCase):
    def job(self, limit=10, reserved=0):
        temporary = tempfile.TemporaryDirectory(prefix="offline-retention-")
        self.addCleanup(temporary.cleanup)
        events = []
        gh, ge = Source(events), Destination(events)
        for tag in ("v0.3.1", "v0.5.0", "v0.7.2"):
            source = gh.add(tag)
            if tag != "v0.7.2":
                ge.add(source, gh)
        job = sync.Sync("serylane", gh, ge, Path(temporary.name), max_total_bytes=limit, other_attachment_bytes=reserved)
        return job, gh, ge, events

    def run_job(self, job, apply=True, focused=None, keep=1):
        with patch.object(job, "sync_refs", return_value="mirror"), patch.object(sync, "git_run", return_value="a" * 40), patch("builtins.print"):
            job.run("all", apply, focused, keep)

    def test_latest_means_numeric_stable_version_not_api_order_or_preview(self):
        gh = Source([])
        gh.add("v0.10.0")
        gh.add("v0.9.9")
        gh.add("v1.0.0", draft=True)
        gh.add("v2.0.0", prerelease=True)
        gh.add("v3.0.0-beta")
        gh.add("v00.9.0")
        self.assertEqual([row["tag_name"] for row in sync.stable_releases(gh.releases)], ["v0.10.0", "v0.9.9"])
        gh.releases.append(deepcopy(gh.releases[0]))
        with self.assertRaisesRegex(sync.SyncError, "Ambiguous"):
            sync.stable_releases(gh.releases)

    def test_plan_frees_only_needed_oldest_version_before_new_upload(self):
        job, gh, ge, events = self.job()
        with patch("builtins.print"):
            plan = job.retention_plan(gh.releases, 1)
        self.assertEqual([group["source"]["tag_name"] for group in plan["before"]], ["v0.3.1"])
        self.assertEqual([group["source"]["tag_name"] for group in plan["after"]], ["v0.5.0"])
        self.assertEqual(events, [])

    def test_retention_dry_run_never_downloads_pushes_uploads_or_deletes(self):
        job, gh, ge, events = self.job()
        with patch.object(job, "sync_refs") as refs, patch("builtins.print"):
            job.run("all", False, keep_latest_releases=1)
        refs.assert_not_called()
        self.assertEqual(events, [])
        self.assertEqual(len(ge.assets[101]), 1)

    def test_complete_retention_verifies_all_bytes_first_and_preserves_metadata(self):
        job, gh, ge, events = self.job()
        self.run_job(job)
        deleted = [event for event in events if event[0] == "delete"]
        self.assertEqual(deleted, [("delete", 101, 110), ("delete", 102, 120)])
        first_delete = events.index(deleted[0])
        for check in (("source-download", 30), ("source-download", 10), ("source-download", 20),
                      ("destination-download", 110), ("destination-download", 120)):
            self.assertLess(events.index(check), first_delete)
        self.assertLess(next(index for index, event in enumerate(events) if event[0] == "upload"), events.index(deleted[1]))
        self.assertEqual(set(ge.releases), {101, 102, 501})
        self.assertEqual(ge.assets[101], [])
        self.assertEqual(ge.assets[102], [])
        self.assertEqual(len(ge.assets[501]), 1)
        self.assertEqual(len(gh.releases), 3)
        self.assertEqual(len(gh.assets), 3)

    def test_scheduled_and_old_focused_runs_never_restore_expired_assets(self):
        job, gh, ge, events = self.job()
        self.run_job(job)
        events.clear()
        self.run_job(job, focused="v0.3.1")
        self.run_job(job)
        self.assertFalse(any(event[0] in {"upload", "delete", "create"} for event in events))
        self.assertEqual(ge.assets[101], [])

    def test_latest_two_retention_does_not_delete_previous_version(self):
        job, gh, ge, events = self.job(limit=20)
        self.run_job(job, keep=2)
        self.assertEqual([event[1] for event in events if event[0] == "delete"], [101])
        self.assertEqual(len(ge.assets[102]), 1)

    def test_protected_target_only_files_and_releases_still_count_toward_quota(self):
        job, gh, ge, events = self.job(limit=20, reserved=2)
        ge.releases[200] = {"id": 200, "tag_name": "custom-build", "prerelease": False}
        ge.assets[200] = [{"id": 250, "name": "custom.zip", "size": 3}]
        ge.assets[101].append({"id": 260, "name": "notes-only.txt", "size": 2})
        self.run_job(job)
        self.assertEqual(ge.assets[101], [{"id": 260, "name": "notes-only.txt", "size": 2}])
        self.assertEqual(len(ge.assets[200]), 1)
        job.max_total_bytes = 10  # retained 4 + protected 5 + reserved 2 = 11
        with self.assertRaisesRegex(sync.SyncError, "protected files need 11"), patch("builtins.print"):
            job.retention_plan(gh.releases, 1)

    def test_no_deletion_if_retained_version_cannot_fit_or_exceeds_file_limit(self):
        for total, per_file in ((3, 100), (10, 3)):
            job, gh, ge, events = self.job(limit=total)
            job.max_asset_bytes = per_file
            with self.assertRaises(sync.SyncError):
                self.run_job(job)
            self.assertEqual(events, [])

    def test_empty_draft_or_missing_newest_source_does_not_trigger_cleanup(self):
        for variant in ("empty", "draft"):
            job, gh, ge, events = self.job()
            if variant == "empty":
                gh.assets[3] = []
            else:
                for release in gh.releases:
                    release["draft"] = True
            with self.assertRaises(sync.SyncError):
                self.run_job(job)
            self.assertEqual(events, [])

    def test_mismatched_backup_size_or_bytes_stops_before_any_deletion(self):
        for variant in ("size", "source-bytes", "destination-bytes"):
            job, gh, ge, events = self.job()
            if variant == "size":
                ge.assets[102][0]["size"] = 5
            elif variant == "source-bytes":
                gh.contents[20] = b"evil"
            else:
                ge.contents[120] = b"evil"
            with self.assertRaises(sync.SyncError):
                self.run_job(job)
            self.assertFalse(any(event[0] in {"delete", "upload", "create"} for event in events))

    def test_failed_new_upload_keeps_previous_mirror_when_space_allows(self):
        job, gh, ge, events = self.job()
        ge.upload_error = True
        with self.assertRaisesRegex(sync.SyncError, "Upload failed"):
            self.run_job(job)
        self.assertEqual(ge.assets[101], [])
        self.assertEqual(len(ge.assets[102]), 1)

    def prepared_group(self):
        job, gh, ge, events = self.job()
        with patch("builtins.print"):
            plan = job.retention_plan(gh.releases, 1)
            group = plan["before"][0]
            job.verify_retirement(group)
        events.clear()
        return job, gh, ge, events, plan, group

    def test_source_withdrawal_replacement_or_newer_release_stops_cleanup(self):
        for variant in ("withdrawn", "replaced", "newer"):
            job, gh, ge, events, plan, group = self.prepared_group()
            if variant == "withdrawn":
                gh.releases = gh.releases[1:]
            elif variant == "replaced":
                gh.assets[1][0]["digest"] = "sha256:" + "0" * 64
            else:
                gh.add("v0.7.3")
            with self.assertRaises(sync.SyncError), patch("builtins.print"):
                job.retire_attachments(plan, group)
            self.assertEqual(events, [])

    def test_changed_destination_id_is_not_deleted_even_with_same_name_and_size(self):
        job, gh, ge, events, plan, group = self.prepared_group()
        ge.assets[101][0]["id"] = 999
        with self.assertRaisesRegex(sync.SyncError, "attachment changed"), patch("builtins.print"):
            job.retire_attachments(plan, group)
        self.assertEqual(events, [])

    def test_cleanup_requires_verified_backup_marker(self):
        job, gh, ge, events, plan, group = self.prepared_group()
        del group["assets"][0]["sha256"]
        with self.assertRaisesRegex(sync.SyncError, "byte-identical"), patch("builtins.print"):
            job.retire_attachments(plan, group)
        self.assertEqual(events, [])

    def test_uncertain_delete_is_read_back_and_never_blindly_retried(self):
        for removed in (False, True):
            job, gh, ge, events, plan, group = self.prepared_group()
            ge.delete_error, ge.remove_on_error = True, removed
            with patch("builtins.print"):
                if removed:
                    job.retire_attachments(plan, group)
                else:
                    with self.assertRaisesRegex(sync.SyncError, "outcome uncertain"):
                        job.retire_attachments(plan, group)
            self.assertEqual([event for event in events if event[0] == "delete"], [("delete", 101, 110)])

    def test_opaque_source_digest_needs_fresh_bytes_again_before_deletion(self):
        job, gh, ge, events = self.job()
        del gh.assets[1][0]["digest"]
        self.run_job(job)
        self.assertGreaterEqual(events.count(("source-download", 10)), 2)

    def test_retention_count_is_explicit_and_rejects_unsafe_values(self):
        job, gh, ge, events = self.job()
        for count in (0, -1, 11, True, "1"):
            with self.assertRaisesRegex(sync.SyncError, "between 1 and 10"):
                self.run_job(job, keep=count)
        self.assertEqual(events, [])

    def test_delete_api_accepts_204_but_only_for_scoped_release_attachment_ids(self):
        api = sync.Api("gitee", "offline-secret")
        response = BytesIO(b"")
        response.status = 204
        class Opener:
            def open(self, request, timeout):
                self.request = request
                return response
        opener = Opener()
        api.opener = opener
        self.assertIsNone(api.request("/repos/cmmuu/serylane/releases/12/attach_files/34", "DELETE"))
        self.assertNotIn("offline-secret", opener.request.full_url)
        for path in ("/repos/cmmuu/serylane/releases/12", "/repos/cmmuu/serylane/tags/v1.0.0",
                     "/repos/other/serylane/releases/12/attach_files/34", "/repos/cmmuu/serylane/releases/0/attach_files/34"):
            with self.assertRaisesRegex(sync.SyncError, "restricted"):
                api.request(path, "DELETE")

    def test_single_file_guard_uses_100_mib_and_cached_backup_cannot_bypass_override(self):
        job, gh, ge, events = self.job()
        gh.assets[3][0]["size"] = 100_235_768
        self.assertEqual(sync.GE_MAX_ASSET, 104_857_600)
        job.source_assets(gh.releases[2])
        job.max_asset_bytes = 100_000_000
        with self.assertRaisesRegex(sync.SyncError, "per-file limit"):
            job.source_assets(gh.releases[2])


if __name__ == "__main__":
    unittest.main()
