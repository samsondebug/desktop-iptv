# Reference product teardown notes (research only — do not copy branding/assets)

Facts verified from the reference product's own installer/runtime on 2026-09-27, added to the
handoff's "verified" list (CLAUDE.md §1).

## Diagnostics JSON from the shipping app (v1.9.26, Win32)

```json
{
  "app_version": "1.9.26",
  "platform": "Win32",
  "licensing_channel": "sideload",
  "telemetry_enabled": true,
  "discord_presence_enabled": false,
  "webview_storage_health": "initialized",
  "webview_native_probe_succeeded": true,
  "webview_isolated_profile": false,
  "webview_udf_override": false,
  "webview_inprivate_override": false,
  "trakt_identity_ready": true,
  "render_diagnostics": { "schema": 1, "enabled": false, "samples": [] }
}
```

What it tells us:

* **WebView2-hosted UI.** `webview_storage_health`, `webview_native_probe_succeeded`,
  `webview_isolated_profile`, `webview_udf_override` (user-data-folder), `webview_inprivate_override`
  are WebView2 host concepts. The UI is an Edge/Chromium webview with mpv underneath — the same
  shape as our Tauri v2 stack. This validates "Tauri + libmpv" as a like-for-like architecture,
  not a downgrade.
* **`licensing_channel: "sideload"`** — the direct download is licensed differently from the
  Microsoft Store / Mac App Store builds. Our `LicenseStateResponse.tier` + machine-bound token
  covers the sideload path; store IAP is a later channel (CLAUDE.md §10).
* **`telemetry_enabled: true` by default** on the sideload build. We ship analytics **off**
  (CLAUDE.md §11) — a stated differentiator.
* **`trakt_identity_ready`** — a Trakt integration exists (scrobbling / continue-watching sync).
  Not in our v1 scope; VOD progress is local SQLite (`progress` table).
* **`discord_presence_enabled`** — Discord Rich Presence toggle exists (off). Not in v1.
* **`render_diagnostics` with `samples[]`** — they sample render stats into a diagnostics blob.
  Our equivalent is `EngineTelemetryEvent` + the sanitized report in the Diagnostics panel.

## Installer

`IPTV Player Zero_1.9.26_x64-setup.exe` (~113 MB) matches the "direct build v1.9.26" figure in
CLAUDE.md §1. Not unpacked further; nothing from it is used in this repository.

## Engineering Blueprint deck (Sept 2026)

Reviewed page-by-page against CLAUDE.md: same two profile presets, same ERD, same split-canvas
IA, same HWND/NSView embed mandate and <800 ms zap target. No deltas to fold in.
