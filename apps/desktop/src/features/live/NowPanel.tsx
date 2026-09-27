/**
 * Right panel: the "Now / Next" card slot (EPG lands in days 15–30). Until then it shows
 * the current channel, live engine telemetry and the zap timer — the day-1 exit tests.
 */
import { useApp } from "../../lib/store";
import { shortCodec } from "../player/Hud";

function Stat({ k, v, warn }: { k: string; v: string; warn?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-2 py-0.5">
      <span style={{ color: "var(--text-faint)", fontSize: 11.5 }}>{k}</span>
      <span style={{ fontFamily: "var(--mono)", fontSize: 12, color: warn ? "var(--warn)" : "var(--text)" }}>{v}</span>
    </div>
  );
}

export default function NowPanel() {
  const current = useApp((s) => s.currentChannel);
  const t = useApp((s) => s.telemetry);
  const zap = useApp((s) => s.lastZapMs);
  const playback = useApp((s) => s.playback);
  const boot = useApp((s) => s.boot)!;
  const buffering = useApp((s) => s.buffering);

  return (
    <div
      className="opaque border-l shrink-0 flex flex-col"
      style={{ width: "var(--side-w)", borderColor: "var(--border)" }}
    >
      <div className="p-3 border-b" style={{ borderColor: "var(--border)" }}>
        <div style={{ color: "var(--text-faint)", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }}>Now playing</div>
        {current ? (
          <>
            <div className="text-[15px] font-semibold mt-1 leading-tight">{current.name}</div>
            <div style={{ color: "var(--text-dim)", fontSize: 12 }}>{current.group_title ?? "—"}</div>
            {current.tvg_id && (
              <div style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11 }} className="mt-1 truncate">
                tvg-id {current.tvg_id}
              </div>
            )}
          </>
        ) : (
          <div className="mt-1" style={{ color: "var(--text-faint)" }}>
            Nothing playing
          </div>
        )}
      </div>

      <div className="p-3 border-b" style={{ borderColor: "var(--border)" }}>
        <div style={{ color: "var(--text-faint)", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }} className="mb-1">
          Guide
        </div>
        <div className="rounded-md p-3" style={{ background: "var(--bg-elev-2)", color: "var(--text-faint)", fontSize: 12 }}>
          Now / Next and the EPG grid arrive in days 15–30 (XMLTV import + offset + overrides).
        </div>
      </div>

      <div className="p-3 flex-1 overflow-auto">
        <div style={{ color: "var(--text-faint)", fontSize: 11, textTransform: "uppercase", letterSpacing: "0.06em" }} className="mb-1">
          Engine
        </div>
        <Stat k="backend" v={boot.engine_kind} warn={boot.engine_kind === "stub"} />
        <Stat k="profile" v={playback?.profile ?? "—"} />
        <Stat k="zap" v={zap != null ? `${zap} ms` : "—"} warn={zap != null && zap > 800} />
        <Stat k="state" v={buffering ? "buffering" : current ? (playback?.paused ? "paused" : "playing") : "idle"} warn={buffering} />
        {t && (
          <>
            <Stat k="video" v={t.width ? `${t.width}×${t.height} ${shortCodec(t.codec_name)}` : "—"} />
            <Stat k="fps" v={t.fps ? t.fps.toFixed(2) : "—"} />
            <Stat k="bitrate" v={t.bitrate_kbps ? `${(t.bitrate_kbps / 1000).toFixed(2)} Mbps` : "—"} />
            <Stat k="cache" v={`${t.cache_duration_secs.toFixed(1)} s`} warn={t.cache_duration_secs < 1 && !!current && !buffering} />
            <Stat k="dropped" v={String(t.dropped_frames)} warn={t.dropped_frames > 0} />
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
          <span><span className="kbd">⏎</span> play</span>
          <span><span className="kbd">f</span> fullscreen</span>
          <span><span className="kbd">m</span> mute</span>
          <span><span className="kbd">p</span> profile</span>
        </div>
      </div>
    </div>
  );
}
