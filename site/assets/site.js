/* SKTV site: fills download buttons from the latest GitHub release.
   Works on any static host; the /download/<os> short links are handled by api/download.js on Vercel. */
(function () {
  var REPO = "samsondebug/desktop-iptv";
  var RELEASES = "https://github.com/" + REPO + "/releases/latest";
  var ua = navigator.userAgent || "";
  var plat = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  var os = /Windows/i.test(ua) || /Win/i.test(plat) ? "windows"
         : /Mac/i.test(plat) || /Macintosh/i.test(ua) ? "mac"
         : /Linux/i.test(plat) || /Linux/i.test(ua) ? "linux" : "other";
  // Apple Silicon Macs report "MacIntel" in Safari; we cannot tell them apart reliably, so the
  // mac card shows both buttons and the primary button picks Apple Silicon (most Macs sold since 2020).
  var KEYS = {
    "win-exe":       { re: /-setup\.exe$/i,            label: "Windows installer (.exe)",          os: "windows", rank: 0 },
    "win-msi":       { re: /\.msi$/i,                  label: "Windows (.msi)",                    os: "windows", rank: 1 },
    "mac-arm":       { re: /(aarch64|arm64).*\.dmg$/i, label: "macOS · Apple Silicon (.dmg)",      os: "mac",     rank: 0 },
    "mac-x64":       { re: /(x64|x86_64).*\.dmg$/i,    label: "macOS · Intel (.dmg)",              os: "mac",     rank: 1 },
    "linux-appimage":{ re: /\.appimage$/i,             label: "Linux AppImage",                    os: "linux",   rank: 0 },
    "linux-deb":     { re: /\.deb$/i,                  label: "Linux .deb (Debian / Ubuntu)",      os: "linux",   rank: 1 },
    "linux-rpm":     { re: /\.rpm$/i,                  label: "Linux .rpm (Fedora / openSUSE)",    os: "linux",   rank: 2 }
  };
  function mb(n) { return n >= 1e6 ? (n / 1048576).toFixed(0) + " MB" : (n / 1024).toFixed(0) + " KB"; }
  function classify(name) {
    for (var k in KEYS) if (KEYS[k].re.test(name)) return k;
    return null;
  }
  function osLabel(o) { return o === "windows" ? "Windows" : o === "mac" ? "macOS" : o === "linux" ? "Linux" : ""; }
  function $(id) { return document.getElementById(id); }
  function all(sel) { return Array.prototype.slice.call(document.querySelectorAll(sel)); }

  document.documentElement.setAttribute("data-os", os);
  var primary = $("dl-primary"), plabel = $("dl-primary-label");
  if (plabel) plabel.textContent = os === "other" ? "Download" : "Download for " + osLabel(os);
  all("[data-mine-os]").forEach(function (el) { if (el.getAttribute("data-mine-os") === os) el.classList.add("mine"); });

  fetch("https://api.github.com/repos/" + REPO + "/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { if (!r.ok) throw new Error(r.status); return r.json(); })
    .then(function (rel) {
      var byKey = {};
      (rel.assets || []).forEach(function (a) { var k = classify(a.name); if (k && !byKey[k]) byKey[k] = { name: a.name, url: a.browser_download_url, size: a.size, k: k }; });
      var ver = (rel.tag_name || "").replace(/^v/, "");
      all("[data-version]").forEach(function (el) { el.textContent = ver ? "v" + ver : ""; });
      all("[data-date]").forEach(function (el) { el.textContent = rel.published_at ? new Date(rel.published_at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : ""; });
      all("[data-notes]").forEach(function (el) { el.href = rel.html_url || RELEASES; });
      // Per-asset buttons: <a data-asset="win-exe">
      all("[data-asset]").forEach(function (el) {
        var a = byKey[el.getAttribute("data-asset")];
        if (!a) { el.style.display = "none"; return; }
        el.href = a.url;
        var sz = el.querySelector(".sz"); if (sz) sz.textContent = mb(a.size);
        el.title = a.name;
      });
      // Primary button: the visitor's OS, else Windows, else anything.
      var order = os === "windows" ? ["win-exe", "win-msi"] : os === "mac" ? ["mac-arm", "mac-x64"] : os === "linux" ? ["linux-appimage", "linux-deb"] : [];
      var pick = null;
      order.concat(["win-exe", "mac-arm", "linux-appimage"]).some(function (k) { if (byKey[k]) { pick = byKey[k]; return true; } return false; });
      if (primary && pick) {
        primary.href = pick.url;
        if (plabel) plabel.textContent = "Download for " + osLabel(KEYS[pick.k].os) + (pick.k === "mac-arm" ? " (Apple Silicon)" : "");
        var m = $("dl-primary-meta"); if (m) m.textContent = "v" + ver + " · " + mb(pick.size) + " · " + pick.name;
      }
    })
    .catch(function () {
      // Rate-limited or offline: leave the static fallbacks (they point at the releases page).
      all("[data-version]").forEach(function (el) { el.textContent = "latest"; });
    });
})();
