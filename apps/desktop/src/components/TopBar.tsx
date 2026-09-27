import { useEffect, useState } from "react";
import { useApp } from "../lib/store";

function Clock() {
  const [now, setNow] = useState(new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000 * 15);
    return () => clearInterval(t);
  }, []);
  return (
    <span style={{ fontFamily: "var(--mono)", color: "var(--text-dim)" }}>
      {now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
    </span>
  );
}

function TierBadge() {
  const license = useApp((s) => s.license);
  if (!license) return null;
  const hoursLeft = license.expires_at ? Math.max(0, Math.round((license.expires_at * 1000 - Date.now()) / 36e5)) : null;
  const label =
    license.tier === "pro_lifetime" ? "PRO" : license.tier === "trial" ? (hoursLeft != null ? `TRIAL · ${hoursLeft}h` : "TRIAL") : "FREE";
  const color = license.tier === "pro_lifetime" ? "var(--accent-2)" : license.tier === "trial" ? "var(--accent)" : "var(--text-faint)";
  return (
    <span
      title={license.tier === "trial" ? "Full PRO features for 72 hours after your first import" : undefined}
      style={{
        border: `1px solid ${color}`,
        color,
        borderRadius: 4,
        padding: "1px 6px",
        fontSize: 10.5,
        fontWeight: 700,
        letterSpacing: "0.08em",
      }}
    >
      {label}
    </span>
  );
}

export default function TopBar() {
  const config = useApp((s) => s.config)!;
  const playlists = useApp((s) => s.playlists);
  const activePlaylistId = useApp((s) => s.activePlaylistId);
  const selectPlaylist = useApp((s) => s.selectPlaylist);
  const setUi = useApp((s) => s.setUi);
  const imports = useApp((s) => s.imports);
  const importing = Object.values(imports)[0];

  return (
    <div
      className="opaque flex items-center gap-3 px-3 border-b shrink-0"
      style={{ height: "var(--topbar-h)", borderColor: "var(--border)" }}
    >
      <Clock />
      <TierBadge />
      {!config.hide_brand_chrome && (
        <span className="font-semibold tracking-tight" style={{ color: "var(--text)" }}>
          desktop-iptv
        </span>
      )}

      <div className="flex-1 flex items-center justify-center gap-1">
        <div className="seg">
          <button className="on">Live</button>
          {!config.hide_vod_tabs && (
            <>
              <button disabled title="Days 31–50">
                Movies
              </button>
              <button disabled title="Days 31–50">
                Series
              </button>
            </>
          )}
        </div>
      </div>

      <div className="flex items-center gap-2">
        {importing && (
          <span style={{ color: "var(--text-dim)", fontFamily: "var(--mono)", fontSize: 11 }}>
            {importing.stage === "fetching"
              ? "fetching…"
              : `importing ${importing.channels.toLocaleString()}${importing.message ? " · " + importing.message : ""}`}
          </span>
        )}
        <select
          className="input"
          style={{ width: 200 }}
          value={activePlaylistId ?? ""}
          onChange={(e) => void selectPlaylist(e.target.value ? Number(e.target.value) : null)}
        >
          {playlists.length === 0 && <option value="">No playlist</option>}
          {playlists.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name} · {p.channel_count.toLocaleString()}
            </option>
          ))}
        </select>
        <button className="btn" onClick={() => setUi({ addPlaylistOpen: true })} title="Add an M3U playlist">
          + Playlist
        </button>
        <button className="btn ghost" onClick={() => setUi({ diagnosticsOpen: true })} title="Diagnostics (Shift+D)">
          Diag
        </button>
        <button className="btn ghost" onClick={() => setUi({ settingsOpen: true })} title="Settings (Shift+S)">
          ⚙
        </button>
      </div>
    </div>
  );
}
