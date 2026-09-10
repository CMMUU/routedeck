# Serylane publishing and synchronization

## Required for every subsequent release

- Future Serylane implementation/update requests use this complete publication flow by default; explicit analysis-only, local-only or no-release requests are exceptions.

- Treat implementation, publication and synchronization as one delivery: tests → reviewed GitHub main → verified multi-platform release → matching Gitee branches/tags and release attachments → official website copy/docs/deployment → public end-to-end verification.
- Never report a release complete while required Gitee attachment verification or official website deployment/verification is pending or failed. Correct the failure and rerun the existing idempotent pipeline; do not lower SHA-256/signature checks or rewrite an existing release tag.
- Verify GitHub/Gitee version and ref identity, signed update manifests, installer availability and the website's six current-version download routes. Record explicit OS login/reboot acceptance separately.
- macOS static and dynamic tray icons must share the application S artwork; test both normal-font and fallback renderers. Template coloring may adapt to the menu bar, but a separate M/letter glyph is not a brand source.


- Every app/repository/release synchronization includes the official website: review public copy and docs for affected features, run website tests, deploy trusted `main`, then verify the live build and latest-download routes. CI deployment does not automatically rewrite feature descriptions.
- Keep application, repository, Releases display titles, download labels and website branding consistent as Serylane. Never rewrite historical tags, signed binaries, updater identities or real old asset filenames merely to change display branding. Use asset `label` for historical display names.
- macOS downloads must distinguish Intel 芯片 (x64) from Apple 芯片（M 系列）(ARM64). Preserve six platform/architecture routes and never substitute another architecture or an old version when the current package cannot be verified.
- `Sync GitHub to Gitee` calls website checks/deployment after a successful non-dry-run sync. A missing deployment secret or failed live verification means synchronization is incomplete; report it explicitly.
- Production: `cd website && pnpm run deploy` uses version upload/deploy only against `serylane-website` on the existing `serylane.cmmuu.com` binding. Do not run `wrangler deploy`/`triggers deploy` to update this site. Do not change DNS or other sites. Commit and push reviewed source first. Keep the explicit `run`: `pnpm deploy` is a different built-in workspace command.
- Never claim an unreleased source version is the latest downloadable app. Latest installer links resolve verified public release metadata at click time; source deployment is separate from publishing application bundles.
- Preserve routing off by default and separate save/start/Codex-access confirmations. Website testing does not authorize starting, stopping, installing or changing the user's running app or proxy.
- Credentials belong only in the authorized credential store/GitHub Actions secrets. Never put tokens, subscription URLs or user config in source, public artifacts, logs or chat.
