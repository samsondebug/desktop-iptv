/**
 * The player pane: a transparent rectangle over the native libmpv surface, plus HTML overlay
 * chrome. No video pixel ever touches this DOM (CLAUDE.md non-negotiable 2). The pane reports
 * its rect to Rust so mpv places the video inside it (`video-margin-ratio-*`).
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ipc } from "../../lib/ipc";
import { fmtDuration, useApp } from "../../lib/store";
import Hud from "./Hud";

const CHROME_HIDE_MS = 2600;

export default function PlayerPane() {
  const ref = useRef<HTMLDivElement>(null);
  const boot = useApp((s) => s.boot)!;
  const config = useApp((s) => s.config)!;
  const ui = useApp((s) => s.ui);
  const setUi = useApp((s) => s.setUi);
  const currentChannel = useApp((s) => s.currentChannel);
  const currentVod = useApp((s) => s.currentVod);
  const currentEpisode = useApp((s) => s.currentEpisode);
  const currentSeries = useApp((s) => s.currentSeries);
  const telemetry = useApp((s) => s.telemetry);
  const playback = useApp((s) => s.playback);
  const isVod = !!playback?.is_vod;
  // "current" = anything playing; the label depends on what it is.
  const current = currentChannel ?? currentVod ?? currentEpisode ?? (playback && playback.item.kind === "url" ? { id: -1, name: "URL probe" } : null);
  const label = currentChannel
    ? currentChannel.name
    : currentVod
      ? currentVod.title
      : currentEpisode
        ? `${currentSeries?.title ?? "Series"} · S${currentEpisode.season}E${currentEpisode.episode}${currentEpisode.title ? " · " + currentEpisode.title : ""}`
        : "URL probe";
  const sublabel = currentChannel?.group_title ?? currentVod?.category ?? null;
  const buffering = useApp((s) => s.buffering);
  const lastError = useApp((s) => s.lastError);
  const favoriteIds = useApp((s) => s.favoriteIds);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const togglePause = useApp((s) => s.togglePause);
  const toggleMute = useApp((s) => s.toggleMute);
  const setVolume = useApp((s) => s.setVolume);
  const setProfile = useApp((s) => s.setProfile);
  const stop = useApp((s) => s.stop);
  const mini = useApp((s) => s.mini);
  const setMini = useApp((s) => s.setMini);
  const openPane = useApp((s) => s.openPane);
  const activeRecordings = useApp((s) => s.activeRecordings);
  const [chromeVisible, setChromeVisible] = useState(true);
  const hideTimer = useRef<number | null>(null);

  const stub = boot.engine_kind === "stub";

  // ---- report the pane rect to the engine ------------------------------------------
  const report = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    void ipc.setVideoRect(r.left, r.top, r.width, r.height, window.innerWidth, window.innerHeight).catch(() => {});
  }, []);

  useLayoutEffect(() => {
    report();
    const ro = new ResizeObserver(() => report());
    if (ref.current) ro.observe(ref.current);
    window.addEventListener("resize", report);
    // Layout settles a frame after fullscreen/rail changes.
    const t = window.setTimeout(report, 60);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", report);
      window.clearTimeout(t);
    };
  }, [report, ui.fullscreen]);

  // ---- chrome auto-hide -------------------------------------------------------------
  const poke = useCallback(() => {
    setChromeVisible(true);
    if (hideTimer.current) window.clearTimeout(hideTimer.current);
    hideTimer.current = window.setTimeout(() => setChromeVisible(false), CHROME_HIDE_MS);
  }, []);
  useEffect(() => {
    poke();
    return () => {
      if (hideTimer.current) window.clearTimeout(hideTimer.current);
    };
  }, [poke, currentChannel?.id, currentVod?.id, currentEpisode?.id]);

  const isFav = currentChannel ? favoriteIds.has(currentChannel.id) : false;
  const profile = playback?.profile ?? config.default_profile;
  const showChrome = chromeVisible || !current;

  return (
    <div
      ref={ref}
      className={"relative flex-1 min-w-0 min-h-0 select-none " + (showChrome ? "" : "chrome-hidden")}
      style={{ background: stub ? "var(--bg-elev)" : "transparent" }}
      onMouseMove={poke}
      onMouseLeave={() => setChromeVisible(false)}
      onDoubleClick={() => setUi({ fullscreen: !ui.fullscreen })}
      data-player-surface
    >
      {/* Empty / stub states */}
      {!current && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <div className="text-center" style={{ color: "var(--text-faint)" }}>
            {stub ? (
              <>
                <div className="text-[14px] font-semibold" style={{ color: "var(--warn)" }}>
                  Stub engine — libmpv not found
                </div>
                <div className="mt-1 max-w-[520px]" style={{ fontSize: 12 }}>
                  {boot.engine_description}
                </div>
                <div className="mt-1" style={{ fontSize: 12 }}>
                  Drop <code>libmpv-2.dll</code> into <code>src-tauri/lib/</code> (Windows) or install libmpv, then restart.
                </div>
              </>
            ) : (
              <div style={{ fontSize: 13 }}>Pick a channel · press <span className="kbd">/</span> to search</div>
            )}
          </div>
        </div>
      )}

      {/* Buffering */}
      {current && buffering && (
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <div className="flex flex-col items-center gap-2">
            <div className="spinner" />
            <div className="hud">buffering…</div>
          </div>
        </div>
      )}

      {/* Error banner */}
      {current && lastError && !buffering && (
        <div className="absolute left-3 right-3 bottom-14 fade-chrome">
          <div className="hud" style={{ borderLeft: "3px solid var(--danger)" }}>
            {lastError}
          </div>
        </div>
      )}

      {/* Mini mode: drag handle + exit */}
      {mini && (
        <div className="absolute top-0 left-0 right-0 flex items-center gap-1 px-2 fade-chrome" style={{ height: 30, background: "linear-gradient(rgba(0,0,0,0.6), transparent)" }} data-tauri-drag-region>
          <span className="hud" data-tauri-drag-region style={{ fontFamily: "var(--font)", fontSize: 11 }}>
            {label}
          </span>
          <div className="flex-1" data-tauri-drag-region />
          <button className="btn ghost" style={{ color: "white", padding: "0 6px" }} onClick={() => void setMini(false)} title="Back to the full window">
            ⤢
          </button>
        </div>
      )}

      {/* Top-left: channel identity */}
      {current && !mini && (
        <div className="absolute top-3 left-3 flex items-center gap-2 fade-chrome">
          {!isVod && <span className="badge-live">LIVE</span>}
          <span className="hud" style={{ fontFamily: "var(--font)", fontSize: 13, fontWeight: 600 }}>
            {label}
          </span>
          {sublabel && (
            <span className="hud" style={{ color: "var(--text-dim)" }}>
              {sublabel}
            </span>
          )}
        </div>
      )}

      {/* Top-right: HUD (not in mini mode — no room) */}
      {current && !mini && (config.hud_enabled || (chromeVisible && !ui.fullscreen)) && (
        <div className="absolute top-3 right-3 fade-chrome">
          <Hud />
        </div>
      )}

      {/* Bottom: transport */}
      {current && (
        <div className="absolute left-0 right-0 bottom-0 px-3 pb-2 pt-8 fade-chrome" style={{ background: "linear-gradient(transparent, rgba(0,0,0,0.65))" }}>
          {isVod && telemetry && telemetry.duration_s > 0 && (
            <div className="flex items-center gap-2 mb-1">
              <span className="hud">{fmtDuration(telemetry.time_pos_s)}</span>
              <input
                type="range"
                min={0}
                max={Math.floor(telemetry.duration_s)}
                value={Math.floor(telemetry.time_pos_s)}
                onChange={(e) => void ipc.seek(Number(e.target.value)).catch(() => {})}
                className="flex-1"
                style={{ accentColor: "var(--accent)" }}
              />
              <span className="hud">{fmtDuration(telemetry.duration_s)}</span>
            </div>
          )}
          <div className="flex items-center gap-2">
            <button className="btn ghost" style={{ color: "white" }} onClick={() => void togglePause()} title="Pause / resume (space)">
              {playback?.paused ? "▶" : "❚❚"}
            </button>
            <button className="btn ghost" style={{ color: "white" }} onClick={() => void stop()} title="Stop">
              ■
            </button>
            <button className="btn ghost" style={{ color: "white" }} onClick={() => void toggleMute()} title="Mute (m)">
              {playback?.muted || playback?.volume === 0 ? "🔇" : "🔊"}
            </button>
            {!mini && (
              <>
                <input
                  type="range"
                  min={0}
                  max={130}
                  value={playback?.muted ? 0 : (playback?.volume ?? 100)}
                  onChange={(e) => void setVolume(Number(e.target.value))}
                  style={{ width: 120, accentColor: "var(--accent)" }}
                  title="Volume (up to 130%)"
                />
                <span className="hud" style={{ minWidth: 44, textAlign: "center" }}>
                  {playback?.muted ? "mute" : `${playback?.volume ?? 100}%`}
                </span>
              </>
            )}

            <div className="flex-1" />

            {!isVod && !mini && (
              <div className="seg" title="Playback profile (p): Low Latency = 3 s cache for sports · Stable = 20–60 s cache for bad Wi-Fi">
                <button className={profile === "low_latency" ? "on" : ""} onClick={() => void setProfile("low_latency")}>
                  Low latency
                </button>
                <button className={profile === "stable" ? "on" : ""} onClick={() => void setProfile("stable")}>
                  Stable
                </button>
              </div>
            )}
            {currentChannel && !mini && (
              <button className="btn ghost" style={{ color: isFav ? "var(--warn)" : "white" }} onClick={() => void toggleFavorite(currentChannel)} title="Favorite">
                {isFav ? "★" : "☆"}
              </button>
            )}
            {currentChannel && !mini && (
              <button
                className="btn ghost"
                style={{ color: activeRecordings > 0 ? "var(--live)" : "white" }}
                onClick={() => window.dispatchEvent(new CustomEvent("diptv:record", { detail: currentChannel }))}
                title={activeRecordings > 0 ? `Recording (${activeRecordings} active) — open Library to stop` : "Record this channel (PRO)"}
              >
                ●
              </button>
            )}
            {!mini && (
              <button className="btn ghost" style={{ color: "white" }} onClick={() => void setMini(true)} title="Mini player (always on top)">
                ⧉
              </button>
            )}
            {!mini && !isVod && (
              <button className="btn ghost" style={{ color: "white" }} onClick={() => void openPane(currentChannel?.id ?? null)} title="Open another pane (multiscreen, PRO)">
                ⊞
              </button>
            )}
            {!mini && (
              <button className="btn ghost" style={{ color: "white" }} onClick={() => void ipc.openInExternalPlayer().catch(() => {})} title="Open in external player (mpv/VLC via the OS)">
                ↗
              </button>
            )}
            {!mini && (
              <button className="btn ghost" style={{ color: "white" }} onClick={() => setUi({ fullscreen: !ui.fullscreen })} title="Fullscreen (f)">
                {ui.fullscreen ? "⤡" : "⤢"}
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
