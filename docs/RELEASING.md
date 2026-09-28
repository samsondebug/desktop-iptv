# Releasing desktop-iptv

How a version gets from `main` to a link friends can click.

## The pieces

| Piece | Where | What it does |
|---|---|---|
| `release.yml` | `.github/workflows/` | On a `v*` tag: builds Windows (NSIS + MSI), macOS (Apple Silicon + Intel DMG) and Linux (deb + AppImage), signs the updater artifacts, publishes a GitHub release with `latest.json`. |
| `pages.yml` | `.github/workflows/` | Publishes `site/` to GitHub Pages → `https://samsondebug.github.io/desktop-iptv/`. |
| `site/index.html` | repo | The download page. Reads the latest release from the GitHub API, picks the visitor's OS, shows the legal block and install notes. Static — host it anywhere (Pages, Vercel, your own domain). |
| Updater | `tauri-plugin-updater`, `src/lib/updater.ts` | Installed copies fetch `https://github.com/samsondebug/desktop-iptv/releases/latest/download/latest.json` 12 s after start and every 6 h, verify the minisign signature against `plugins.updater.pubkey` in `tauri.conf.json`, and offer "Install & restart" (Settings → About has a manual check). |

## What exists (set up 2026-09-28)

| Thing | Value |
|---|---|
| GitHub repo | `samsondebug/desktop-iptv` (public) — Pages source: GitHub Actions |
| Download page | https://samsondebug.github.io/desktop-iptv/ |
| Azure subscription | `desktop-iptv` (pay-as-you-go, MCA billing account "David Krouskoff") |
| Artifact Signing account | `krouskoffsigning`, resource group `desktop-iptv-signing`, East US, Basic — endpoint `https://eus.codesigning.azure.net` |
| Identity validation | Individual, Public — `CN=David Krouskoff, L=Austin, S=TX, C=US` |
| Certificate profile | `desktop-iptv-public` (Public Trust) |
| CI service principal | Entra app registration `desktop-iptv-ci` — role *Artifact Signing Certificate Profile Signer* on the account; client secret expires 2028-09-28 |
| GitHub secrets | `TAURI_SIGNING_PRIVATE_KEY`, `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_PROFILE` |

Gotchas met on the way: a Free Trial subscription that was "upgraded" keeps `quotaId FreeTrial_…`
for up to a day and Artifact Signing rejects it — a fresh subscription under the same billing
profile is pay-as-you-go immediately. GitHub Actions on a private repo needs billing set up on the
account; public repos build for free. `actions/configure-pages` cannot create the Pages site with
`GITHUB_TOKEN` — set Settings → Pages → Source → *GitHub Actions* once by hand.

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

## Code signing (Windows) — Azure Artifact Signing

`release.yml` signs the exe and the installers automatically once five repository secrets exist;
without them it builds unsigned. Artifact Signing (Microsoft's renamed Trusted Signing) is a
managed CA: Basic tier ≈ US$10/month for 5,000 signatures, no key to hold, certificates rotate
every three days and are timestamped (`timestamp.acs.microsoft.com`) so signatures stay valid.

Getting the account (individual developer, must be in the US or Canada):

1. Azure subscription on **Pay-As-You-Go** (free/trial subscriptions are refused). The billing
   account must be of type *Individual* and its legal name + sold-to address must match the
   government ID used later — fix that under Cost Management + Billing before step 4.
2. Subscription → Resource providers → register `Microsoft.CodeSigning`.
3. Create an **Artifact Signing account** (portal: "Artifact Signing Accounts" → Create): new
   resource group, a globally unique name, region (East US = `https://eus.codesigning.azure.net`),
   pricing tier **Basic**. Billing starts at creation and is not pro-rated.
4. Account → Access control (IAM) → assign yourself **Artifact Signing Identity Verifier**, then
   Identity validations → *Individual* → New Identity → Public. Pick the billing account; the form
   fills itself. When it flips to *Action Required*, follow the link: email PIN, phone, then AU10TIX
   scans your driver's licence/passport + a selfie on your phone, and Microsoft Authenticator
   stores a Verified ID you present back to the portal. Usually minutes; up to 20 business days if
   documents are requested.
5. Certificate profiles → Create → **Public Trust**, pick the validated identity. This is the
   `AZURE_SIGNING_PROFILE` name; the account name is `AZURE_SIGNING_ACCOUNT`.
6. A service principal for CI: Microsoft Entra ID → App registrations → New (any name) → note the
   *Application (client) ID* and *Directory (tenant) ID*; Certificates & secrets → New client
   secret → copy the value. On the Artifact Signing account → IAM → assign that app
   **Artifact Signing Certificate Profile Signer**.
7. GitHub → Settings → Secrets → Actions: `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`,
   `AZURE_CLIENT_SECRET`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` (and
   `AZURE_SIGNING_ENDPOINT` if the region is not East US). Next tag → signed installers.

Locally, the same signing works with `cargo install artifact-signing-cli`, the three `AZURE_*` env
vars, and `tauri build --config '{"bundle":{"windows":{"signCommand":"artifact-signing-cli -e <endpoint> -a <account> -c <profile> -d desktop-iptv %1"}}}'`.

SmartScreen reputation is per certificate and builds with downloads; a freshly issued certificate
can still show the "More info → Run anyway" prompt for the first days. Submitting a signed build
at microsoft.com/wdsi speeds that up.

## What users see until the builds are code-signed

* **Windows**: SmartScreen "Windows protected your PC" → *More info → Run anyway*; Edge/Chrome may
  flag the download as uncommon → *Keep*. Fix: the section above.
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
