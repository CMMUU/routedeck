# Serylane installer identity

The NSIS template is derived from Tauri CLI 2.11.4:
https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.11.4/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi
(Apache-2.0 OR MIT). Keep the upstream version pinned when updating it.

Local changes preserve the old, internal NSIS registration keys while displaying
Serylane and installing serylane.exe. They also recognize the legacy MSI display
name, update only shortcuts targeting this installation's old executable, and
migrate only an existing, exactly matching login registration. A fresh install
does not enable login startup. Upgrades keep the old installation directory;
moving it could break shortcuts, permissions and rollback.

WiX upgrade code is pinned to the output of `tauri inspect wix-upgrade-code`
before the product rename: 3dc8b957-21f1-51b9-ab4c-df67ce49b53d.
Application identifier, data directory, helper identity and signing key remain
unchanged. Old updater clients still require RouteDeck-named signed aliases;
these are byte-identical copies, not separately built executables.

The WiX `login-cleanup.wxs` fragment invokes an exact, UI-free application
command before a normal uninstall, never during an upgrade. It removes only
current-user login commands pointing exactly at this installation's executable;
saved settings, proxy state and other copies are left alone. Cleanup is best
effort so a damaged/missing executable cannot trap users in an uninstall failure.

The release workflow runs `scripts/test-windows-installers.ps1` after building
on disposable GitHub-hosted Windows x64/ARM64 machines. It checks NSIS and MSI
fresh installation, same-directory upgrade from checksum-verified 0.7.6,
settings/shortcut preservation, owned login-entry migration, quiet `--autostart`
versus visible manual launch, unchanged stopped-state proxy and uninstall.
Fixtures include installation paths with spaces, the legacy dependency's
unquoted startup commands, and Task Manager-disabled startup approvals. Migration
normalizes owned command quoting without resetting those approvals.
The script refuses to run outside the fixed repository's hosted runners or
when pre-existing application data/login entries are present. It does not
perform a physical reboot or claim live TUN/model-stream acceptance.
Do not install test builds on the development user's live proxy machine.
macOS/Linux login-entry migration still requires independent platform acceptance.

For a normal release, dispatch **Release bundles** on `main` with
`publish_current: true` and leave `release_tag` empty. Every platform must pass
before the version tag is created and verified. Existing tags are never moved;
use `release_tag` to repair the same tagged source. The default dispatch remains
a non-publishing build. Signing checks and a complete artifact set are required
before the GitHub draft becomes public; Gitee and website verification follow.
