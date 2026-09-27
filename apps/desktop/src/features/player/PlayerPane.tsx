/**
 * The player pane: a transparent rectangle over the native libmpv surface, plus HTML overlay
 * chrome. No video pixel ever touches this DOM (CLAUDE.md non-negotiable 2). The pane reports
 * its rect to Rust so mpv places the video inside it (`video-margin-ratio-*`).
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ipc } from "../../lib/ipc";
import { useApp } from "../../lib/store";
import Hud from "./Hud";

const CHROME_HIDE_MS = 2600;

export default function PlayerPane() {
  const ref = useRef<HTMLDivElement>(null);
  const boot = useApp((s) => s.boot)!;
  const config = useApp((s) => s.config)!;
  const ui = useApp((s) => s.ui);
  const setUi = useApp((s) => s.setUi);
  const current = useApp((s) => s.currentChannel);
  const playback = useApp((s) => s.playback);
  const buffering = useApp((s) => s.buffering);
  const lastError = useApp((s) => s.lastError);
  const favoriteIds = useApp((s) => s.favoriteIds);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const togglePause = useApp((s) => s.togglePause);
  const toggleMute = useApp((s) => s.toggleMute);
  const setVolume = useApp((s) => s.setVolume);
  const setProfile = useApp((s) => s.setProfile);
  const stop = useApp((s) => s.stop);
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
  }, [poke, current?.id]);

  const isFav = current ? favoriteIds.has(current.id) : false;
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

      {/* Top-left: channel identity */}
      {current && (
        <div className="absolute top-3 left-3 flex items-center gap-2 fade-chrome">
          <span className="badge-live">LIVE</span>
          <span className="hud" style={{ fontFamily: "var(--font)", fontSize: 13, fontWeight: 600 }}>
            {current.name}
          </span>
          {current.group_title && (
            <span className="hud" style={{ color: "var(--text-dim)" }}>
              {current.group_title}
            </span>
          )}
        </div>
      )}

      {/* Top-right: HUD */}
      {current && (config.hud_enabled || (chromeVisible && !ui.fullscreen)) && (
        <div className="absolute top-3 right-3 fade-chrome">
          <Hud />
        </div>
      )}

      {/* Bottom: transport */}
      {current && (
        <div className="absolute left-0 right-0 bottom-0 px-3 pb-2 pt-8 fade-chrome" style={{ background: "linear-gradient(transparent, rgba(0,0,0,0.65))" }}>
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

            <div className="flex-1" />

            <div className="seg" title="Playback profile (p): Low Latency = 3 s cache for sports · Stable = 20–60 s cache for bad Wi-Fi">
              <button className={profile === "low_latency" ? "on" : ""} onClick={() => void setProfile("low_latency")}>
                Low latency
              </button>
              <button className={profile === "stable" ? "on" : ""} onClick={() => void setProfile("stable")}>
                Stable
              </button>
            </div>
            <button
              className="btn ghost"
              style={{ color: isFav ? "var(--warn)" : "white" }}
              onClick={() => current && void toggleFavorite(current)}
              title="Favorite"
            >
              {isFav ? "★" : "☆"}
            </button>
            <button className="btn ghost" style={{ color: "white" }} onClick={() => setUi({ fullscreen: !ui.fullscreen })} title="Fullscreen (f)">
              {ui.fullscreen ? "⤡" : "⤢"}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
