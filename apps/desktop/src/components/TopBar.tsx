import { useEffect, useState } from "react";
import { useApp } from "../lib/store";
import Icon from "./Icon";

function Clock() {
  const [now, setNow] = useState(new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 1000 * 15);
    return () => clearInterval(t);
  }, []);
  return <span style={{ fontFamily: "var(--mono)", color: "var(--text-dim)" }}>{now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>;
}

function TierBadge() {
  const license = useApp((s) => s.license);
  const setUi = useApp((s) => s.setUi);
  if (!license) return null;
  const hoursLeft = license.expires_at ? Math.max(0, Math.round((license.expires_at * 1000 - Date.now()) / 36e5)) : null;
  const label = license.tier === "pro_lifetime" ? "PRO" : license.tier === "trial" ? (hoursLeft != null ? `TRIAL · ${hoursLeft}h` : "TRIAL") : "FREE";
  const color = license.tier === "pro_lifetime" ? "var(--accent-2)" : license.tier === "trial" ? "var(--accent)" : "var(--text-faint)";
  return (
    <span
      title={license.tier === "trial" ? "Full PRO features for 72 hours after your first import" : license.tier === "free" ? "Unlock PRO in Settings → License" : undefined}
      onClick={() => setUi({ settingsOpen: true, settingsTab: "license" })}
      style={{ border: `1px solid ${color}`, color, borderRadius: 4, padding: "1px 6px", fontSize: 11.5, fontWeight: 700, letterSpacing: "0.08em", cursor: "default" }}
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
  const tab = useApp((s) => s.tab);
  const setTab = useApp((s) => s.setTab);
  const imports = useApp((s) => s.imports);
  const importing = activePlaylistId != null ? imports[activePlaylistId] : Object.values(imports)[0];

  const phaseLabel = importing
    ? importing.phase === "account"
      ? "signing in…"
      : importing.phase === "epg"
        ? `guide ${importing.stage === "fetching" ? "fetching…" : `${importing.channels.toLocaleString()} programmes${importing.message ? " · " + importing.message : ""}`}`
        : importing.phase === "vod"
          ? `library ${importing.stage === "fetching" ? "fetching…" : importing.channels.toLocaleString()}`
          : importing.stage === "fetching"
            ? "fetching…"
            : `importing ${importing.channels.toLocaleString()}${importing.message ? " · " + importing.message : ""}`
    : null;

  return (
    <div className="opaque flex items-center gap-3 px-3 border-b shrink-0" style={{ height: "var(--topbar-h)", borderColor: "var(--border)" }}>
      <Clock />
      <TierBadge />
      {!config.hide_brand_chrome && (
        <span className="font-semibold tracking-tight" style={{ color: "var(--text)" }}>
          desktop-iptv
        </span>
      )}

      <div className="flex-1 flex items-center justify-center gap-1">
        <div className="seg nav">
          <button className={tab === "live" ? "on" : ""} onClick={() => setTab("live")} title="Live TV (1)">
            Live
          </button>
          {!config.hide_vod_tabs && (
            <>
              <button className={tab === "movies" ? "on" : ""} onClick={() => setTab("movies")} title="Movies (2)">
                Movies
              </button>
              <button className={tab === "series" ? "on" : ""} onClick={() => setTab("series")} title="Series (3)">
                Series
              </button>
            </>
          )}
        </div>
      </div>

      <div className="flex items-center gap-2">
        {phaseLabel && (
          <span className="flex items-center gap-1.5" style={{ color: "var(--text-dim)", fontFamily: "var(--mono)", fontSize: 11.5 }}>
            <span className="spinner" style={{ width: 12, height: 12, borderWidth: 2 }} />
            {phaseLabel}
          </span>
        )}
        <select className="input" style={{ width: 200 }} value={activePlaylistId ?? ""} onChange={(e) => void selectPlaylist(e.target.value ? Number(e.target.value) : null)}>
          {playlists.length === 0 && <option value="">No playlist</option>}
          {playlists.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name} · {p.channel_count.toLocaleString()}
            </option>
          ))}
        </select>
        <button className="btn" onClick={() => setUi({ addPlaylistOpen: true })} title="Add a playlist">
          <Icon name="plus" size={14} />
          Playlist
        </button>
        <button className="btn ghost" onClick={() => setUi({ diagnosticsOpen: true })} title="Diagnostics (Shift+D)">
          <Icon name="activity" size={16} />
          Diagnostics
        </button>
        <button className="btn ghost icon" onClick={() => setUi({ settingsOpen: true })} title="Settings (Shift+S)" aria-label="Settings">
          <Icon name="settings" size={18} />
        </button>
      </div>
    </div>
  );
}
