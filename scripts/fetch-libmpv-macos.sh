#!/usr/bin/env bash
# Bundle libmpv (and the dylibs it needs) into apps/desktop/src-tauri/lib/ for the macOS build.
# Uses Homebrew's mpv; rewrites install names so the copies resolve relative to each other
# (@loader_path). desktop-iptv searches Contents/Resources/lib at startup.
# The walk is in Python because macOS ships bash 3.2 (no associative arrays).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
lib="$root/apps/desktop/src-tauri/lib"
mkdir -p "$lib"
if [ -f "$lib/libmpv.2.dylib" ]; then echo "libmpv.2.dylib already present"; exit 0; fi
prefix="$(brew --prefix)"
src="$prefix/lib/libmpv.2.dylib"
[ -f "$src" ] || { echo "install mpv first: brew install mpv" >&2; exit 1; }

python3 - "$src" "$lib" "$prefix" <<'PY'
import os, shutil, subprocess, sys
src, lib, prefix = sys.argv[1:4]
roots = (prefix + "/", "/opt/homebrew/", "/usr/local/")

def deps(path):
    out = subprocess.check_output(["otool", "-L", path], text=True).splitlines()[1:]
    return [line.split()[0] for line in out if line.strip()]

# Copy libmpv plus its transitive Homebrew dependencies.
seen, queue = set(), [src]
while queue:
    f = queue.pop(0)
    name = os.path.basename(f)
    if name in seen:
        continue
    seen.add(name)
    real = os.path.realpath(f)
    dst = os.path.join(lib, name)
    shutil.copyfile(real, dst)
    os.chmod(dst, 0o755)
    for d in deps(real):
        if d.startswith(roots):
            queue.append(d)

# Rewrite install names to @loader_path so the bundle is relocatable, then ad-hoc sign.
for name in sorted(os.listdir(lib)):
    if not name.endswith(".dylib"):
        continue
    f = os.path.join(lib, name)
    subprocess.check_call(["install_name_tool", "-id", f"@loader_path/{name}", f])
    for d in deps(f):
        if d.startswith(roots):
            subprocess.check_call(["install_name_tool", "-change", d, f"@loader_path/{os.path.basename(d)}", f])
    subprocess.call(["codesign", "--force", "--sign", "-", f], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
print(f"{len(seen)} dylibs bundled into {lib}")
PY
