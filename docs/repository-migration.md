# Sovelyx Desk repository migration

Canonical repository: https://github.com/bluemoonandy/SovelyxDesk.
This is a standalone repository created independently of GitHub's fork network.
The original fork https://github.com/bluemoonandy/maccanvas remains available with its Actions history and artifacts.

## Git history and integration baseline

All 10 remote branches and the original `v0.5.6` tag were copied without rewriting their commits.
`archive/maccanvas-main` retains the old fork's original `main` at `30c07828e8fe4568300e695a49075de6468df373`.
The integration baseline is `603e584571f70ae1bd14ff7dd4f02a0cb83b2931` on `codex/p3-brand-i18n`:
Sovelyx branding, Chinese/English UI, device management, Fit/Zoom/DPI fixes, native clipboard/file transfers and clipboard diagnostics.
The migration changes build on that baseline on `codex/repository-migration`.
After Windows CI verification, the new repository's `main` can advance to that verified commit. The old fork's branches are not modified.
Historical tags identify their original source; they are not new Sovelyx product releases.

## Compatibility

Existing `%LOCALAPPDATA%\MacCanvas` profiles/preferences and `%USERPROFILE%\.maccanvas` audit logs retain their paths.
Credential Manager targets `MacCanvas/profile/` and `MacCanvas/login/v2/` and Windows Hello checks remain unchanged.
There is no device-data migration or empty-configuration reset in this repository migration.
The installer recognizes the previous Sovelyx installation receipt repository `bluemoonandy/maccanvas` only when its expected executable and SHA-256 still match.
New installation receipts use `bluemoonandy/SovelyxDesk`; foreign or modified receipts are rejected.
Legacy receipt compatibility never permits downloads from the old fork or upstream: executable downloads must belong to the new repository and pass hash validation.

## Builds and publishing

Windows CI performs format, tests, Clippy, installer, UI/DPI/language/fullscreen checks and Release packaging.
Portable bundles include `SovelyxDesk.exe`, the optional branded audio helper, license notices, checksums and the corresponding source archive.
The release workflow prepares a draft; review its assets before publishing. Existing releases cannot be overwritten by that workflow.
`updates_ready=false` remains in `brand/brand.json`: repository migration does not enable automatic downloads or installation.
The product homepage is configured as `https://sovelyx.com/desk`, but website deployment is separate.

## Backup and rollback

The original repository and its branches, tags and CI records are retained.
A full Git bundle and pre-migration reference/metadata snapshots were saved locally before any new repository writes.
GitHub Issues, PRs, Releases, Actions history, Secrets and Variables are not transported by Git; the old fork had no Releases or repository Secrets/Variables at migration time.
Use the preserved prior portable build to roll back the app without deleting profiles or credentials.
The previous local MacCanvas working directory and its uncommitted edits remain separate from the new `SovelyxDesk` checkout.

## Attribution

MacCanvas by @false00 remains the upstream source. `LICENSE`, original copyrights and third-party notices are retained.
Repository independence does not change the project's AGPL-3.0-or-later license or the requirement to supply matching source and build information with distributed versions.
