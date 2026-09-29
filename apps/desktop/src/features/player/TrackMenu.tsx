/**
 * Audio / subtitle track picker (+ VOD playback speed). Reads mpv's `track-list` when opened,
 * selects with `sid`/`aid`. Subtitles ship off (`sid=no` — broken EIA-608 on live streams);
 * turning one on here is an explicit per-session choice.
 */
import { useEffect, useRef, useState } from "react";
import { ipc, type MpvTrack } from "../../lib/ipc";
import Icon from "../../components/Icon";

const SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2];

function trackLabel(t: MpvTrack): string {
  const bits = [t.title, t.lang?.toUpperCase(), t.codec].filter(Boolean);
  return bits.length ? bits.join(" · ") : `Track ${t.id}`;
}

export default function TrackMenu({ isVod }: { isVod: boolean }) {
  const [open, setOpen] = useState(false);
  const [tracks, setTracks] = useState<MpvTrack[]>([]);
  const [speed, setSpeed] = useState(1);
  const ref = useRef<HTMLDivElement>(null);

  const refresh = () => {
    void ipc
      .engineTracks()
      .then((t) => setTracks(Array.isArray(t) ? t : []))
      .catch(() => setTracks([]));
    if (isVod) void ipc.engineGetProperty("speed").then((v) => setSpeed(Number(v) || 1)).catch(() => {});
  };

  useEffect(() => {
    if (open) refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Close on outside click / Escape.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const audio = tracks.filter((t) => t.type === "audio");
  const subs = tracks.filter((t) => t.type === "sub");
  const subOn = subs.some((t) => t.selected);

  const select = (kind: "audio" | "sub", id: string) => {
    void ipc
      .engineSelectTrack(kind, id)
      .then(() => setTimeout(refresh, 150))
      .catch(() => {});
  };

  const setPlaybackSpeed = (s: number) => {
    setSpeed(s);
    void ipc.engineSetProperty("speed", String(s)).catch(() => {});
  };

  return (
    <div className="relative" ref={ref}>
      <button
        className="btn ghost icon"
        style={{ color: subOn || (isVod && speed !== 1) ? "var(--accent)" : "white" }}
        onClick={() => setOpen((o) => !o)}
        title="Audio & subtitle tracks"
        aria-label="Audio and subtitle tracks"
        aria-expanded={open}
      >
        <Icon name="captions" size={17} />
      </button>
      {open && (
        <div
          className="absolute panel rounded-md py-1 z-50"
          style={{ bottom: 36, right: 0, minWidth: 240, maxHeight: 320, overflowY: "auto" }}
          onClick={(e) => e.stopPropagation()}
        >
          <div className="px-3 py-1" style={{ color: "var(--text-faint)", fontSize: 11, fontWeight: 600, letterSpacing: "0.06em" }}>
            AUDIO
          </div>
          {audio.length === 0 && (
            <div className="px-3 py-1" style={{ color: "var(--text-faint)", fontSize: 12 }}>
              No audio tracks reported
            </div>
          )}
          {audio.map((t) => (
            <Row key={`a${t.id}`} label={trackLabel(t)} on={t.selected} onClick={() => select("audio", String(t.id))} />
          ))}

          <div className="px-3 py-1 mt-1" style={{ color: "var(--text-faint)", fontSize: 11, fontWeight: 600, letterSpacing: "0.06em" }}>
            SUBTITLES
          </div>
          <Row label="Off" on={!subOn} onClick={() => select("sub", "no")} />
          {subs.map((t) => (
            <Row key={`s${t.id}`} label={trackLabel(t)} on={t.selected} onClick={() => select("sub", String(t.id))} />
          ))}
          {subs.length === 0 && (
            <div className="px-3 py-1" style={{ color: "var(--text-faint)", fontSize: 12 }}>
              This stream carries no subtitle tracks
            </div>
          )}

          {isVod && (
            <>
              <div className="px-3 py-1 mt-1" style={{ color: "var(--text-faint)", fontSize: 11, fontWeight: 600, letterSpacing: "0.06em" }}>
                SPEED
              </div>
              <div className="px-3 py-1 flex flex-wrap gap-1">
                {SPEEDS.map((s) => (
                  <button key={s} className={"btn ghost" + (speed === s ? " on" : "")} style={{ fontSize: 11.5, padding: "2px 8px", color: speed === s ? "var(--accent)" : undefined }} onClick={() => setPlaybackSpeed(s)}>
                    {s}×
                  </button>
                ))}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}

function Row({ label, on, onClick }: { label: string; on: boolean; onClick: () => void }) {
  return (
    <div
      className="px-3 py-1.5 cursor-default flex items-center gap-2"
      style={{ fontSize: 12.5 }}
      onClick={onClick}
      onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-elev-2)")}
      onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}
    >
      <span style={{ width: 14, display: "inline-flex" }}>{on && <Icon name="check" size={13} style={{ color: "var(--accent)" }} />}</span>
      <span className="truncate">{label}</span>
    </div>
  );
}
