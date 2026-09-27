import { useEffect, useMemo, useState } from "react";
import { ipc, type EpisodeRecord, type SeriesDetail } from "../../lib/ipc";
import { fmtDuration, useApp } from "../../lib/store";

export default function SeriesModal({ seriesId }: { seriesId: number }) {
  const setUi = useApp((s) => s.setUi);
  const playEpisode = useApp((s) => s.playEpisode);
  const current = useApp((s) => s.currentEpisode);
  const [detail, setDetail] = useState<SeriesDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [season, setSeason] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const close = () => setUi({ seriesOpen: null });

  const load = async (force: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const d = await ipc.seriesDetail(seriesId, force);
      setDetail(d);
      if (season == null) setSeason(d.episodes[0]?.season ?? null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  useEffect(() => {
    void load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [seriesId]);

  const seasons = useMemo(() => Array.from(new Set(detail?.episodes.map((e) => e.season) ?? [])).sort((a, b) => a - b), [detail]);
  const progress = useMemo(() => new Map(detail?.progress.map((p) => [p.item_id, p]) ?? []), [detail]);
  const eps = detail?.episodes.filter((e) => e.season === season) ?? [];
  const nextUnwatched = detail?.episodes.find((e) => !progress.get(e.id)?.finished) ?? null;

  const pctOf = (e: EpisodeRecord) => {
    const p = progress.get(e.id);
    if (!p) return 0;
    if (p.finished) return 1;
    const d = p.duration_s ?? e.duration ?? 0;
    return d > 0 ? Math.min(1, p.position_s / d) : 0;
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: "min(860px, 94vw)", maxHeight: "88vh" }}>
        {!detail ? (
          <div className="flex items-center gap-3">
            <div className="spinner" />
            <div style={{ color: "var(--text-dim)" }}>{error ?? "Fetching episodes from your provider…"}</div>
            {error && (
              <button className="btn" onClick={() => void load(true)}>
                Retry
              </button>
            )}
            <div className="flex-1" />
            <button className="btn ghost" onClick={close}>
              ✕
            </button>
          </div>
        ) : (
          <>
            <div className="flex gap-4">
              {detail.series.poster && (
                <img src={detail.series.poster} alt="" style={{ width: 140, aspectRatio: "2/3", objectFit: "cover", borderRadius: 8, background: "var(--bg-elev-2)" }} draggable={false} onError={(e) => ((e.target as HTMLImageElement).style.visibility = "hidden")} />
              )}
              <div className="min-w-0 flex-1">
                <div className="flex items-start gap-2">
                  <div className="text-[18px] font-semibold leading-tight flex-1">{detail.series.title}</div>
                  <button className="btn ghost" onClick={() => void load(true)} title="Refresh episodes" disabled={busy}>
                    ↻
                  </button>
                  <button className="btn ghost" onClick={close}>
                    ✕
                  </button>
                </div>
                <div className="mt-1" style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11 }}>
                  {[detail.series.year, detail.series.genre, detail.series.rating ? `★ ${detail.series.rating.toFixed(1)}` : null, `${detail.episodes.length} episodes`, detail.series.category].filter(Boolean).join(" · ")}
                </div>
                {detail.series.description && (
                  <div className="mt-2" style={{ color: "var(--text-dim)", fontSize: 12.5, lineHeight: 1.5, maxHeight: 96, overflow: "auto" }}>
                    {detail.series.description}
                  </div>
                )}
                <div className="mt-3 flex gap-2">
                  {nextUnwatched && (
                    <button className="btn primary" onClick={() => void playEpisode(nextUnwatched, detail.series)}>
                      ▶ {progress.get(nextUnwatched.id) ? "Continue" : "Play"} S{nextUnwatched.season}E{nextUnwatched.episode}
                    </button>
                  )}
                </div>
              </div>
            </div>

            {seasons.length > 1 && (
              <div className="tabs mt-4">
                {seasons.map((s) => (
                  <button key={s} className={s === season ? "on" : ""} onClick={() => setSeason(s)}>
                    Season {s}
                  </button>
                ))}
              </div>
            )}
            <div className="mt-2 flex flex-col gap-0.5" style={{ maxHeight: 360, overflow: "auto" }}>
              {eps.map((e) => {
                const pct = pctOf(e);
                const isCur = current?.id === e.id;
                return (
                  <div key={e.id} className="episode-row" onDoubleClick={() => void playEpisode(e, detail.series)}>
                    <div style={{ fontFamily: "var(--mono)", color: isCur ? "var(--accent)" : "var(--text-dim)", fontSize: 12 }}>
                      S{e.season}E{e.episode}
                    </div>
                    <div className="min-w-0">
                      <div className="truncate" style={{ color: isCur ? "var(--accent)" : "var(--text)" }}>
                        {e.title ?? `Episode ${e.episode}`}
                      </div>
                      {pct > 0 && (
                        <div className="progress-bar mt-1" style={{ width: 160 }}>
                          <div style={{ width: `${pct * 100}%` }} />
                        </div>
                      )}
                    </div>
                    <div style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11 }}>{e.duration ? fmtDuration(e.duration) : ""}</div>
                    <div className="flex gap-1">
                      <button className="btn" onClick={() => void playEpisode(e, detail.series)}>
                        ▶ {pct > 0 && pct < 1 ? "Resume" : "Play"}
                      </button>
                      {pct > 0 && (
                        <button className="btn ghost" title="Clear progress" onClick={() => void ipc.clearProgress("episode", e.id).then(() => load(false))}>
                          ↺
                        </button>
                      )}
                    </div>
                  </div>
                );
              })}
              {eps.length === 0 && <div style={{ color: "var(--text-faint)" }}>No episodes listed by the provider.</div>}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
