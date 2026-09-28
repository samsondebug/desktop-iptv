# Releasing desktop-iptv

How a version gets from `main` to a link friends can click.

## The pieces

| Piece | Where | What it does |
|---|---|---|
| `release.yml` | `.github/workflows/` | On a `v*` tag: builds Windows (NSIS + MSI), macOS (Apple Silicon + Intel DMG) and Linux (deb + AppImage), signs the updater artifacts, publishes a GitHub release with `latest.json`. |
| `pages.yml` | `.github/workflows/` | Publishes `site/` to GitHub Pages → `https://samsondebug.github.io/desktop-iptv/`. |
| `site/index.html` | repo | The download page. Reads the latest release from the GitHub API, picks the visitor's OS, shows the legal block and install notes. Static — host it anywhere (Pages, Vercel, your own domain). |
| Updater | `tauri-plugin-updater`, `src/lib/updater.ts` | Installed copies fetch `https://github.com/samsondebug/desktop-iptv/releases/latest/download/latest.json` 12 s after start and every 6 h, verify the minisign signature against `plugins.updater.pubkey` in `tauri.conf.json`, and offer "Install & restart" (Settings → About has a manual check). |

## One-time setup (already done except the secret)

1. **Updater keypair.** Generated with `npx tauri signer generate`. The public key is committed in
   `apps/desktop/src-tauri/tauri.conf.json`. The private key is **not** in the repo — add it as the
   repository secret `TAURI_SIGNING_PRIVATE_KEY` (Settings → Secrets and variables → Actions → New
   repository secret; paste the whole key string). No password was set, so
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` stays unset. Losing the private key means installed copies
   can never update again (they would need a fresh install with a new public key) — keep a copy.
2. **Public repo.** GitHub Pages on the free plan and `releases/latest/download/…` links for people
   who are not signed in both need the repo to be public. Actions minutes are also free on public
   repos (private: 2,000 min/month with macOS billed 10×; one release ≈ 350 min).
3. **No runtime prerequisites on Windows.** `tauri-build` links `vcruntime` statically, so the exe
   imports only system DLLs plus the UCRT (`api-ms-win-crt-*`, part of Windows 10/11) — verified
   with `pefile` on the shipped build; no VC++ redistributable needed. libmpv-2.dll (MinGW build)
   imports only system DLLs; WebView2 is installed by the NSIS bootstrapper when missing.

## Shipping a version

```
# bump the version in Cargo.toml [workspace.package] (all crates + tauri.conf.json read it),
# apps/desktop/package.json and apps/desktop/src-tauri/tauri.conf.json — keep all three equal
git commit -am "0.1.1"
git tag -a v0.1.1 -m "desktop-iptv 0.1.1"
git push origin main v0.1.1
```

About 25 minutes later the release is live at `github.com/samsondebug/desktop-iptv/releases/latest`,
the download page shows it, and every installed copy offers the update at its next check. A tag
with a `-` in it (e.g. `v0.2.0-beta.1`) is published as a pre-release, which `latest` ignores.

Manual runs (`workflow_dispatch`) build without publishing and attach the bundles as workflow
artifacts — useful to test a build on another machine first.

## What users see until the builds are code-signed

* **Windows**: SmartScreen "Windows protected your PC" → *More info → Run anyway*; Edge/Chrome may
  flag the download as uncommon → *Keep*. Fix: sign the installer. Cheapest credible route is Azure
  Trusted Signing (identity validation + ~US$10/month); an OV certificate from a CA works too. Tauri
  takes either through `bundle.windows.signCommand` / `certificateThumbprint` — wire it in
  `release.yml` next to the Apple secrets.
* **macOS**: "cannot be opened because the developer cannot be verified" → System Settings →
  Privacy & Security → *Open Anyway*. Fix: an Apple Developer account (US$99/year); set the
  `APPLE_*` secrets already referenced in `release.yml` and tauri-action signs + notarizes.
* **Linux**: no gate.

## Hosting the page somewhere other than GitHub Pages

`site/index.html` is one file with no build step. Drop it on any static host or behind your own
domain; it keeps pulling release metadata from the GitHub API, which allows 60 unauthenticated
requests per hour per visitor IP — plenty for a download page. If the repo is private the API
returns 404 and the page falls back to a plain link to the releases page (which then also needs a
GitHub login), so make the repo public first.

## Local cross-build from Linux (what produced the first installers)

```
cargo install cargo-xwin && rustup target add x86_64-pc-windows-msvc && sudo apt install nsis
export TAURI_SIGNING_PRIVATE_KEY=$(cat /path/to/desktop-iptv.key)
cd apps/desktop && npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
# → target/x86_64-pc-windows-msvc/release/bundle/nsis/desktop-iptv_<v>_x64-setup.exe (+ .sig)
```
