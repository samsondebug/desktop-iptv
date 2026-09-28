// sktv.app/download/<os> → 302 to the newest matching installer on GitHub Releases.
// Runs as a Vercel serverless function (vercel.json rewrites /download/windows etc. here).
// The redirect is cached at Vercel's edge for 10 minutes, so GitHub's unauthenticated API limit
// (60 requests/hour per IP) is never a problem no matter how many people click the link.
const REPO = "samsondebug/desktop-iptv";
const RELEASES = `https://github.com/${REPO}/releases/latest`;
const PATTERNS = {
  windows: /_x64-setup\.exe$/i,
  "windows-msi": /_x64.*\.msi$/i,
  mac: /(aarch64|arm64).*\.dmg$/i,
  "mac-intel": /(x64|x86_64).*\.dmg$/i,
  linux: /\.AppImage$/i,
  "linux-deb": /\.deb$/i,
  "linux-rpm": /\.rpm$/i,
};

module.exports = async (req, res) => {
  const os = String(req.query.os || "").toLowerCase();
  const re = PATTERNS[os];
  res.setHeader("Cache-Control", "public, s-maxage=600, stale-while-revalidate=3600");
  if (!re) {
    res.statusCode = 302;
    res.setHeader("Location", "/download/");
    return res.end();
  }
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`, {
      headers: { Accept: "application/vnd.github+json", "User-Agent": "sktv-site" },
    });
    if (!r.ok) throw new Error(`github ${r.status}`);
    const rel = await r.json();
    const asset = (rel.assets || []).find((a) => re.test(a.name));
    if (!asset) throw new Error("no asset");
    res.statusCode = 302;
    res.setHeader("Location", asset.browser_download_url);
    return res.end();
  } catch (e) {
    // Rate-limited or GitHub down: send them to the releases page rather than a 500.
    res.setHeader("Cache-Control", "public, s-maxage=60");
    res.statusCode = 302;
    res.setHeader("Location", RELEASES);
    return res.end();
  }
};
