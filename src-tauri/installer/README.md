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

Before publishing, test fresh install and upgrades from 0.7.6 in disposable
Windows x64/ARM64 machines: NSIS and MSI, with/without login startup, shortcuts,
silent login, application update, and uninstall. Do not install test builds on
the development user's live proxy machine. macOS/Linux packaging and login-entry
migration also require platform verification before claiming support.
