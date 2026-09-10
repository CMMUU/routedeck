import copy
import unittest
from release_display import asset_label, display_body, immutable_asset, normalize


class FakeApi:
    def __init__(self):
        self.release = {'id': 42, 'tag_name': 'v0.7.6', 'name': 'RouteDeck v0.7.6', 'body': '# RouteDeck v0.7.6\n', 'draft': False, 'prerelease': False}
        self.asset = {'id': 88, 'name': 'RouteDeck_0.7.6_aarch64.dmg', 'size': 123, 'digest': 'sha256:' + 'a' * 64, 'state': 'uploaded', 'label': '', 'browser_download_url': 'https://github.com/CMMUU/serylane/releases/download/v0.7.6/RouteDeck_0.7.6_aarch64.dmg'}
        self.writes = []

    def pages(self, path):
        return copy.deepcopy([self.asset] if path.endswith('/assets') else [self.release])

    def request(self, path, method='GET', data=None):
        if path == '/repos/CMMUU/serylane':
            return {'id': 1355770287}
        target = self.asset if '/assets/' in path else self.release
        if method != 'GET':
            assert method == 'PATCH'
            assert set(data) == ({'label'} if target is self.asset else {'name', 'body'})
            self.writes.append((path, copy.deepcopy(data)))
            target.update(data)
        return copy.deepcopy(target)


class DisplayTests(unittest.TestCase):
    def test_labels_preserve_chip_and_package_type(self):
        self.assertIn('Intel 芯片', asset_label('RouteDeck_0.7.6_x64.dmg'))
        self.assertIn('Apple 芯片（M 系列）', asset_label('mihomo-codex_0.5.0_aarch64.dmg'))
        self.assertIn('Windows · x64 · EXE', asset_label('Serylane_0.7.7_x64-setup.exe'))
        self.assertIn('Linux · ARM64 · rpm', asset_label('RouteDeck-0.7.6-1.aarch64.rpm'))
        self.assertTrue(asset_label('RouteDeck_0.7.6_aarch64.app.tar.gz.sig').endswith('更新签名'))
        self.assertIsNone(asset_label('mihomo-v1.19.30-source.tar.gz'))
        self.assertIsNone(asset_label('Mihomo-LICENSE-v1.19.30.txt'))
        self.assertIsNone(asset_label('latest.json'))
        self.assertIsNone(asset_label('Serylane_0.7.7_universal.dmg'))

    def test_notes_idempotent_and_preserve_real_paths(self):
        source = '# RouteDeck v0.7.6\n使用 RouteDeck；内部标识 `routedeck`\nhttps://github.com/CMMUU/mihomo-codex/releases/download/v0.7.6/RouteDeck_0.7.6_x64-setup.exe\n'
        result = display_body(source)
        self.assertIn('# Serylane v0.7.6', result)
        self.assertIn('`routedeck`', result)
        self.assertIn('/CMMUU/serylane/releases/download/v0.7.6/RouteDeck_0.7.6_x64-setup.exe', result)
        self.assertEqual(display_body(result), result)
        self.assertIn('```sh\nRouteDeck --help\n```', display_body('```sh\nRouteDeck --help\n```'))

    def test_preview_and_apply_never_change_immutable_asset_fields(self):
        api = FakeApi()
        original = immutable_asset(api.asset)
        normalize(api)
        self.assertEqual(api.writes, [])
        normalize(api, apply=True)
        self.assertEqual(api.release['name'], 'Serylane v0.7.6')
        self.assertEqual(immutable_asset(api.asset), original)
        self.assertEqual(len(api.writes), 2)
        normalize(api, apply=True)
        self.assertEqual(len(api.writes), 2)

    def test_concurrent_asset_change_stops_before_patch(self):
        api = FakeApi()
        request = api.request
        def mutate(path, method='GET', data=None):
            if '/assets/' in path and method == 'GET':
                api.asset['size'] += 1
            return request(path, method, data)
        api.request = mutate
        with self.assertRaisesRegex(ValueError, 'Asset changed'):
            normalize(api, apply=True)
        self.assertTrue(all('/assets/' not in path for path, _ in api.writes))

    def test_wrong_repository_cannot_be_modified(self):
        api = FakeApi()
        api.request = lambda *args: {'id': 1}
        with self.assertRaisesRegex(ValueError, 'identity'):
            normalize(api, apply=True)
        self.assertEqual(api.writes, [])

    def test_gitee_requires_original_tag_and_preserves_release_identity(self):
        class GiteeApi(FakeApi):
            def request(self, path, method='GET', data=None):
                if path == '/repos/cmmuu/serylane':
                    return {'id': 50078322}
                if method != 'GET':
                    assert method == 'PATCH'
                    assert set(data) == {'name', 'body', 'tag_name', 'prerelease'}
                    assert data['tag_name'] == self.release['tag_name']
                    assert data['prerelease'] == str(self.release['prerelease']).lower()
                    self.writes.append((path, copy.deepcopy(data)))
                    self.release.update({**data, 'prerelease': data['prerelease'] == 'true'})
                return copy.deepcopy(self.release)
        github, gitee = FakeApi(), GiteeApi()
        gitee.release['target_commitish'] = 'a' * 40
        normalize(github, gitee, apply=True)
        self.assertEqual(gitee.release['name'], 'Serylane v0.7.6')
        self.assertEqual(gitee.release['tag_name'], 'v0.7.6')
        self.assertEqual(gitee.release['target_commitish'], 'a' * 40)
        self.assertEqual(len(gitee.writes), 1)
        normalize(github, gitee, apply=True)
        self.assertEqual(len(gitee.writes), 1)


if __name__ == '__main__':
    unittest.main()
