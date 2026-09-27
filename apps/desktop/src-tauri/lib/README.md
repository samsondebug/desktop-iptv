# Bundled native libraries

Drop `libmpv-2.dll` (Windows, x86_64, from a static mpv-dev build such as zhongfly/mpv-winbuild
`mpv-dev-lgpl-x86_64-*.7z`) into this folder. It is bundled as a resource and found at runtime by
`app-engine` (search order: `DESKTOP_IPTV_LIBMPV`, exe dir, `lib/`, `resources/lib/`, system path).

macOS: `brew install mpv` (libmpv.2.dylib is found in /opt/homebrew/lib) or copy the dylib here.
Linux: `apt install libmpv2` (or libmpv-dev).

Without libmpv the app still starts with the stub engine (no video) so the catalog UI keeps working.
