<#
.SYNOPSIS
  Download libmpv-2.dll for Windows builds into apps/desktop/src-tauri/lib/ (bundled as a resource).

.DESCRIPTION
  desktop-iptv loads libmpv at runtime (libloading), so the DLL only needs to sit next to the
  executable or under resources/lib. This script fetches a "mpv-dev-x86_64-*.7z" archive from the
  shinchiro/mpv-winbuild-cmake GitHub releases (the de-facto Windows mpv builds), extracts
  libmpv-2.dll and records the source URL + SHA-256 in lib/LIBMPV-SOURCE.txt.

  Override the archive with $env:LIBMPV_DEV_URL (a direct link to a .7z or .zip that contains
  libmpv-2.dll) — e.g. to pin a build for a release.

.NOTES
  Needs 7z on PATH for .7z archives (GitHub runners have it). PowerShell 5+.
#>
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$libDir = Join-Path $root "apps\desktop\src-tauri\lib"
New-Item -ItemType Directory -Force -Path $libDir | Out-Null
$target = Join-Path $libDir "libmpv-2.dll"
if (Test-Path $target) { Write-Host "libmpv-2.dll already present at $target"; exit 0 }

$url = $env:LIBMPV_DEV_URL
if (-not $url) {
  Write-Host "Resolving latest mpv-dev-x86_64 archive from GitHub releases…"
  $headers = @{ "User-Agent" = "desktop-iptv-ci" }
  if ($env:GITHUB_TOKEN) { $headers["Authorization"] = "Bearer $env:GITHUB_TOKEN" }
  $rel = Invoke-RestMethod -Headers $headers -Uri "https://api.github.com/repos/shinchiro/mpv-winbuild-cmake/releases/latest"
  # Prefer the plain x86_64 build (widest CPU support) over the -v3 (AVX2) variant.
  $asset = $rel.assets | Where-Object { $_.name -match '^mpv-dev-x86_64-\d{8}-git-[0-9a-f]+\.7z$' } | Select-Object -First 1
  if (-not $asset) { $asset = $rel.assets | Where-Object { $_.name -match '^mpv-dev-x86_64.*\.7z$' } | Select-Object -First 1 }
  if (-not $asset) { throw "No mpv-dev-x86_64 asset found in release $($rel.tag_name); set LIBMPV_DEV_URL" }
  $url = $asset.browser_download_url
}
Write-Host "Downloading $url"
$tmp = Join-Path $env:TEMP ("libmpv-" + [guid]::NewGuid().ToString() )
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
$archive = Join-Path $tmp ([IO.Path]::GetFileName(($url -split '\?')[0]))
Invoke-WebRequest -Uri $url -OutFile $archive -Headers @{ "User-Agent" = "desktop-iptv-ci" }
if ($archive -like "*.zip") {
  Expand-Archive -Path $archive -DestinationPath (Join-Path $tmp "x") -Force
} else {
  & 7z x -y "-o$(Join-Path $tmp 'x')" $archive | Out-Null
  if ($LASTEXITCODE -ne 0) { throw "7z extraction failed" }
}
$dll = Get-ChildItem -Path (Join-Path $tmp "x") -Recurse -Filter "libmpv-2.dll" | Select-Object -First 1
if (-not $dll) { throw "libmpv-2.dll not found inside $archive" }
Copy-Item $dll.FullName $target -Force
$sha = (Get-FileHash -Algorithm SHA256 $target).Hash
"source: $url`nsha256: $sha`nfetched: $(Get-Date -Format o)" | Set-Content (Join-Path $libDir "LIBMPV-SOURCE.txt")
Write-Host "libmpv-2.dll → $target (sha256 $sha)"
Remove-Item -Recurse -Force $tmp
