#!/usr/bin/env bash
# Bundle libmpv (and the dylibs it needs) into apps/desktop/src-tauri/lib/ for the macOS build.
# Uses Homebrew's mpv; rewrites install names so the copies resolve relative to each other
# (@loader_path). desktop-iptv searches Contents/Resources/lib at startup.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
lib="$root/apps/desktop/src-tauri/lib"
mkdir -p "$lib"
if [ -f "$lib/libmpv.2.dylib" ]; then echo "libmpv.2.dylib already present"; exit 0; fi
prefix="$(brew --prefix)"
src="$prefix/lib/libmpv.2.dylib"
[ -f "$src" ] || { echo "install mpv first: brew install mpv" >&2; exit 1; }

# Copy libmpv plus its transitive Homebrew dependencies.
declare -A seen
queue=("$src")
while ((${#queue[@]})); do
  f="${queue[0]}"; queue=("${queue[@]:1}")
  real="$(python3 -c 'import os,sys;print(os.path.realpath(sys.argv[1]))' "$f")"
  name="$(basename "$f")"
  [[ -n "${seen[$name]:-}" ]] && continue
  seen[$name]=1
  cp -f "$real" "$lib/$name"
  chmod u+w "$lib/$name"
  while read -r dep; do
    case "$dep" in
      "$prefix"/*|/opt/homebrew/*|/usr/local/*) queue+=("$dep") ;;
    esac
  done < <(otool -L "$real" | tail -n +2 | awk '{print $1}')
done

# Rewrite install names to @loader_path so the bundle is relocatable.
for f in "$lib"/*.dylib; do
  install_name_tool -id "@loader_path/$(basename "$f")" "$f"
  while read -r dep; do
    case "$dep" in
      "$prefix"/*|/opt/homebrew/*|/usr/local/*)
        install_name_tool -change "$dep" "@loader_path/$(basename "$dep")" "$f" ;;
    esac
  done < <(otool -L "$f" | tail -n +2 | awk '{print $1}')
  codesign --force --sign - "$f" >/dev/null 2>&1 || true
done
ls -1 "$lib" | wc -l | xargs -I{} echo "{} dylibs bundled into $lib"
