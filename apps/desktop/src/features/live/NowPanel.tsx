/**
 * Right panel: Now / Next card for the playing channel (from the imported guide), VOD info
 * when a movie/episode is playing, engine telemetry and the zap timer.
 */
import { useEffect, useState } from "react";
import { ipc, type NowNext } from "../../lib/ipc";
import { fmtDuration, fmtTime, useApp } from "../../lib/store";
import { shortCodec } from "../player/Hud";

function Stat({ k, v, warn }: { k: string; v: string; warn?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-2 py-0.5">
      <span style={{ color: "var(--text-faint)", fontSize: 11.5 }}>{k}</span>
      <span style={{ fontFamily: "var(--mono)", fontSize: 12, color: warn ? "var(--warn)" : "var(--text)" }}>{v}</span>
    </div>
  );
}

function Section({ title, children, right }: { title: string; children: React.ReactNode; right?: React.ReactNode }) {
  return (
    <div className="p-3 border-b" style={{ borderColor: "var(--border)" }}>
      <div className="flex items-center justify-between mb-1">
        <div style={{ color: "var(--text-faint)", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}>{title}</div>
        {right}
      </div>
      {children}
    </div>
  );
}

export default function NowPanel() {
  const current = useApp((s) => s.currentChannel);
  const currentVod = useApp((s) => s.currentVod);
  const currentEpisode = useApp((s) => s.currentEpisode);
  const currentSeries = useApp((s) => s.currentSeries);
  const t = useApp((s) => s.telemetry);
  const zap = useApp((s) => s.lastZapMs);
  const playback = useApp((s) => s.playback);
  const boot = useApp((s) => s.boot)!;
  const buffering = useApp((s) => s.buffering);
  const epgVersion = useApp((s) => s.epgVersion);
  const epgStats = useApp((s) => s.epgStats);
  const setUi = useApp((s) => s.setUi);
  const [nn, setNn] = useState<NowNext | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    const i = setInterval(() => setTick((x) => x + 1), 30_000);
    return () => clearInterval(i);
  }, []);
  useEffect(() => {
    if (!current) {
      setNn(null);
      return;
    }
    ipc.epgNowNext(current.id).then(setNn).catch(() => setNn(null));
  }, [current, epgVersion, tick]);

  const now = Math.floor(Date.now() / 1000);
  const pct = nn?.now ? Math.min(1, Math.max(0, (now - nn.now.start) / Math.max(1, nn.now.stop - nn.now.start))) : 0;

  return (
    <div className="opaque border-l shrink-0 flex flex-col" style={{ width: "var(--side-w)", borderColor: "var(--border)" }}>
      <Section title={currentVod || currentEpisode ? "Now playing" : "Now playing"}>
        {current ? (
          <>
            <div className="text-[15px] font-semibold mt-1 leading-tight">{current.name}</div>
            <div style={{ color: "var(--text-dim)", fontSize: 12 }}>{current.group_title ?? "—"}</div>
            <div style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11 }} className="mt-1 truncate">
              tvg-id {nn?.tvg_id ?? current.tvg_id ?? "—"}
              {current.catchup_days > 0 && ` · catch-up ${current.catchup_days}d`}
            </div>
          </>
        ) : currentVod ? (
          <>
            <div className="text-[15px] font-semibold mt-1 leading-tight">{currentVod.title}</div>
            <div style={{ color: "var(--text-dim)", fontSize: 12 }}>{[currentVod.year, currentVod.genre, currentVod.category].filter(Boolean).join(" · ")}</div>
          </>
        ) : currentEpisode ? (
          <>
            <div className="text-[15px] font-semibold mt-1 leading-tight">{currentSeries?.title ?? "Series"}</div>
            <div style={{ color: "var(--text-dim)", fontSize: 12 }}>
              S{currentEpisode.season}E{currentEpisode.episode}
              {currentEpisode.title ? ` · ${currentEpisode.title}` : ""}
            </div>
            <button className="btn mt-2" onClick={() => currentSeries && setUi({ seriesOpen: currentSeries.id })}>
              Episodes…
            </button>
          </>
        ) : (
          <div className="mt-1" style={{ color: "var(--text-faint)" }}>
            Nothing playing
          </div>
        )}
      </Section>

      {(current || !currentVod) && !currentEpisode && (
        <Section
          title="Guide"
          right={
            current && (
              <button className="btn ghost" style={{ padding: "0 6px", fontSize: 11 }} onClick={() => setUi({ epgEditChannel: current })} title="Edit EPG (tvg-id) for this channel">
                Edit EPG
              </button>
            )
          }
        >
          {nn?.now || nn?.next ? (
            <div className="flex flex-col gap-2">
              {nn.now && (
                <div>
                  <div className="flex items-baseline justify-between gap-2">
                    <div className="font-semibold truncate">{nn.now.title}</div>
                    <div style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11, whiteSpace: "nowrap" }}>
                      {fmtTime(nn.now.start)}–{fmtTime(nn.now.stop)}
                    </div>
                  </div>
                  <div className="progress-bar mt-1">
                    <div style={{ width: `${pct * 100}%` }} />
                  </div>
                  {nn.now.desc && (
                    <div className="mt-1" style={{ color: "var(--text-dim)", fontSize: 12, lineHeight: 1.45, maxHeight: 88, overflow: "hidden" }}>
                      {nn.now.desc}
                    </div>
                  )}
                </div>
              )}
              {nn.next && (
                <div style={{ color: "var(--text-dim)", fontSize: 12 }}>
                  <span style={{ color: "var(--text-faint)" }}>Next · {fmtTime(nn.next.start)}</span> {nn.next.title}
                </div>
              )}
            </div>
          ) : (
            <div className="rounded-md p-3" style={{ background: "var(--bg-elev-2)", color: "var(--text-faint)", fontSize: 12 }}>
              {!current
                ? epgStats && epgStats.programmes > 0
                  ? `Guide loaded: ${epgStats.programmes.toLocaleString()} programmes for ${epgStats.channels_with_epg.toLocaleString()} channels.`
                  : "No guide yet. Xtream playlists fetch their EPG automatically; for M3U add an XMLTV URL in Settings → Guide."
                : epgStats && epgStats.programmes > 0
                  ? "No programmes for this channel. Its tvg-id may not match the guide — use Edit EPG."
                  : "No guide imported for this playlist."}
            </div>
          )}
        </Section>
      )}

      <div className="p-3 flex-1 overflow-auto">
        <div style={{ color: "var(--text-faint)", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }} className="mb-1">
          Engine
        </div>
        <Stat k="backend" v={boot.engine_kind} warn={boot.engine_kind === "stub"} />
        <Stat k="profile" v={playback?.profile ?? "—"} />
        <Stat k="zap" v={zap != null ? `${zap} ms` : "—"} warn={zap != null && zap > 800} />
        <Stat k="state" v={buffering ? "buffering" : current || currentVod || currentEpisode ? (playback?.paused ? "paused" : "playing") : "idle"} warn={buffering} />
        {t && (
          <>
            <Stat k="video" v={t.width ? `${t.width}×${t.height} ${shortCodec(t.codec_name)}` : "—"} />
            <Stat k="fps" v={t.fps ? t.fps.toFixed(2) : "—"} />
            <Stat k="bitrate" v={t.bitrate_kbps ? `${(t.bitrate_kbps / 1000).toFixed(2)} Mbps` : "—"} />
            <Stat k="cache" v={`${t.cache_duration_secs.toFixed(1)} s`} warn={t.cache_duration_secs < 1 && !!current && !buffering} />
            <Stat k="dropped" v={String(t.dropped_frames)} warn={t.dropped_frames > 0} />
            {playback?.is_vod && t.duration_s > 0 && <Stat k="position" v={`${fmtDuration(t.time_pos_s)} / ${fmtDuration(t.duration_s)}`} />}
          </>
        )}
        <div className="mt-3" style={{ color: "var(--text-faint)", fontSize: 11, lineHeight: 1.5 }}>
          Zap time ≠ live delay. Provider HLS windows are often 6–20 s server-side; Low Latency only shrinks <em>our</em> cache.
        </div>
      </div>

      <div className="p-3 border-t" style={{ borderColor: "var(--border)", color: "var(--text-faint)", fontSize: 11 }}>
        <div className="flex flex-wrap gap-x-3 gap-y-1">
          <span><span className="kbd">/</span> search</span>
          <span><span className="kbd">↑↓</span> move</span>
          <span><span className="kbd">←→</span> {playback?.is_vod ? "seek" : "guide"}</span>
          <span><span className="kbd">⏎</span> play</span>
          <span><span className="kbd">f</span> fullscreen</span>
          <span><span className="kbd">m</span> mute</span>
          <span><span className="kbd">p</span> profile</span>
          <span><span className="kbd">1 2 3</span> tabs</span>
        </div>
      </div>
    </div>
  );
}
