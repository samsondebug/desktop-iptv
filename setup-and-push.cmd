@echo off
setlocal EnableExtensions
rem ---------------------------------------------------------------------------
rem  desktop-iptv — first-run helper for Windows
rem    1. pushes this repo (full git history included) to GitHub as samsondebug/desktop-iptv
rem    2. optionally fetches libmpv-2.dll and starts the dev build
rem  Run it from the extracted folder (double-click is fine).
rem ---------------------------------------------------------------------------
cd /d "%~dp0"
echo.
echo === desktop-iptv setup ===
echo.

where git >nul 2>&1
if errorlevel 1 (
  echo [!] git is not installed. Get it from https://git-scm.com/download/win and run this again.
  goto :end
)

if not exist ".git" (
  echo [!] .git folder missing - this zip should contain the history. Initialising a fresh repo instead.
  git init -b main
  git add -A
  git commit -m "desktop-iptv: initial import" >nul
)

set REPO=samsondebug/desktop-iptv
git remote get-url origin >nul 2>&1
if errorlevel 1 (
  where gh >nul 2>&1
  if errorlevel 1 (
    echo [i] GitHub CLI ^(gh^) not found - falling back to a plain git remote.
    echo     Create an EMPTY private repo named desktop-iptv at https://github.com/new first,
    echo     then press any key to push to https://github.com/%REPO%.git
    pause >nul
    git remote add origin https://github.com/%REPO%.git
  ) else (
    gh auth status >nul 2>&1 || gh auth login
    echo [i] Creating private repo %REPO% and pushing...
    gh repo create %REPO% --private --source=. --remote=origin --push --description "Player-only desktop IPTV client (Tauri v2 + Rust + libmpv + SQLite)"
    if not errorlevel 1 goto :pushed
    echo [!] gh repo create failed ^(repo may already exist^). Trying a plain push...
    git remote get-url origin >nul 2>&1 || git remote add origin https://github.com/%REPO%.git
  )
)
git push -u origin main
if errorlevel 1 (
  echo [!] push failed. If the repo does not exist yet create it at https://github.com/new ^(empty, private^) and rerun.
  goto :end
)
:pushed
echo.
echo [ok] pushed to https://github.com/%REPO%
echo.

choice /C YN /M "Fetch libmpv-2.dll and start the dev build now (needs Rust + Node + VS Build Tools)"
if errorlevel 2 goto :end
where cargo >nul 2>&1 || (echo [!] Rust not found: https://rustup.rs & goto :end)
where npm   >nul 2>&1 || (echo [!] Node.js not found: https://nodejs.org & goto :end)
powershell -ExecutionPolicy Bypass -File scripts\fetch-libmpv.ps1
pushd apps\desktop
call npm install
call npm run tauri dev
popd

:end
echo.
pause
endlocal
