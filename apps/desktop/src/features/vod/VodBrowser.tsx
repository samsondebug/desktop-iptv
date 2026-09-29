/**
 * Movies / Series poster grid (CLAUDE.md §12 days 31–50). Virtualized in rows of N posters,
 * paged from SQLite; lazy posters; continue-watching progress bars.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ipc, type ContinueItem, type VodKind, type VodRecord } from "../../lib/ipc";
import { LIST_ENTER, LIST_MOVE } from "../../lib/keys";
import { fmtDuration, useApp } from "../../lib/store";
import { useVodSource } from "./useVodSource";
import Icon from "../../components/Icon";

const CARD_W = 150;
const CARD_H = 290;

/** Decade filter chips. `to` omitted = open-ended (Older = everything before 2000). */
const DECADES: { label: string; from: number; to?: number }[] = [
  { label: "2020s", from: 2020 },
  { label: "2010s", from: 2010, to: 2019 },
  { label: "2000s", from: 2000, to: 2009 },
  { label: "Older", from: 1900, to: 1999 },
];

export default function VodBrowser({ kind }: { kind: VodKind }) {
  const src = useVodSource(kind);
  const search = useApp((s) => s.search);
  const setSearch = useApp((s) => s.setSearch);
  const category = useApp((s) => s.ui.vodCategory);
  const sort = useApp((s) => s.ui.vodSort);
  const filter = useApp((s) => s.ui.vodFilter);
  const setUi = useApp((s) => s.setUi);
  const playVod = useApp((s) => s.playVod);
  const selectedIndex = useApp((s) => s.ui.selectedIndex);
  const listVersion = useApp((s) => s.listVersion);
  const rail = useApp((s) => s.rail);
  const parentRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(900);
  const [progress, setProgress] = useState<Map<string, number>>(new Map());
  const [cont, setCont] = useState<ContinueItem[]>([]);
  const [menu, setMenu] = useState<{ x: number; y: number; v: VodRecord } | null>(null);
  const pushToast = useApp((s) => s.pushToast);
  useEffect(() => {
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    return () => window.removeEventListener("click", close);
  }, []);

  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    ipc
      .continueWatching()
      .then((items) => {
        setCont(items);
        const m = new Map<string, number>();
        for (const it of items) {
          const d = it.progress.duration_s ?? it.vod?.duration_s ?? it.episode?.duration ?? 0;
          if (d > 0) m.set(`${it.progress.item_type}:${it.progress.item_id}`, it.progress.position_s / d);
        }
        setProgress(m);
      })
      .catch(() => {});
  }, [listVersion]);

  const cols = Math.max(2, Math.floor((width - 16) / CARD_W));
  const rows = Math.ceil(src.count / cols);
  const virtualizer = useVirtualizer({ count: rows, getScrollElement: () => parentRef.current, estimateSize: () => CARD_H, overscan: 2 });
  const items = virtualizer.getVirtualItems();

  useEffect(() => {
    if (items.length) src.ensure(items[0].index * cols, (items[items.length - 1].index + 1) * cols - 1);
  }, [items, src, cols]);

  useEffect(() => {
    const onMove = (e: Event) => {
      const delta = (e as CustomEvent<number>).detail;
      const step = Math.abs(delta) === 12 ? cols * 3 * Math.sign(delta) : delta * cols; // ↑↓ move by row
      const next = Math.max(0, Math.min(src.count - 1, (selectedIndex < 0 ? -1 : selectedIndex) + step));
      setUi({ selectedIndex: next });
      virtualizer.scrollToIndex(Math.floor(next / cols), { align: "auto" });
    };
    const onEnter = () => {
      if (selectedIndex < 0) return;
      const v = src.row(selectedIndex);
      if (v) open(v);
    };
    window.addEventListener(LIST_MOVE, onMove);
    window.addEventListener(LIST_ENTER, onEnter);
    return () => {
      window.removeEventListener(LIST_MOVE, onMove);
      window.removeEventListener(LIST_ENTER, onEnter);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedIndex, src, cols, virtualizer]);

  const open = (v: VodRecord) => {
    if (v.kind === "series") setUi({ seriesOpen: v.id });
    else void playVod(v);
  };

  const continueItems = useMemo(() => cont.filter((c) => (kind === "movie" ? !!c.vod : !!c.episode)), [cont, kind]);
  const showContinue = rail.kind === "continue";

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-2 px-3 py-1.5 border-b shrink-0" style={{ borderColor: "var(--border)" }}>
        <div className="font-semibold truncate">
          {showContinue ? "Continue watching" : search.trim() ? `Search: “${search.trim()}”` : category ?? (kind === "movie" ? "All movies" : "All series")}
        </div>
        <div style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11.5 }}>{showContinue ? continueItems.length : src.loading ? "…" : src.count.toLocaleString()}</div>
        <div className="flex-1" />
        <select className="input" style={{ width: 130 }} value={sort} onChange={(e) => setUi({ vodSort: e.target.value as typeof sort })}>
          <option value="added">Newest</option>
          <option value="title">Title A–Z</option>
          <option value="year">Year</option>
          <option value="rating">Rating</option>
        </select>
        <input
          id="channel-search"
          className="input"
          style={{ width: 280 }}
          placeholder={`Search ${kind === "movie" ? "movies" : "series"}  ( / )`}
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          spellCheck={false}
          autoComplete="off"
        />
      </div>

      {!showContinue && !search.trim() && (
        <div className="flex items-center gap-1.5 px-3 py-1.5 border-b shrink-0 flex-wrap" style={{ borderColor: "var(--border)" }}>
          {DECADES.map((d) => {
            const on = filter.year_from === d.from && (filter.year_to ?? null) === (d.to ?? null);
            return (
              <button
                key={d.label}
                className={"btn ghost" + (on ? " on" : "")}
                style={{ fontSize: 11.5, padding: "2px 10px", color: on ? "var(--accent)" : "var(--text-dim)", border: "1px solid var(--border)", borderRadius: 999 }}
                onClick={() => setUi({ vodFilter: on ? { ...filter, year_from: null, year_to: null } : { ...filter, year_from: d.from, year_to: d.to ?? null } })}
              >
                {d.label}
              </button>
            );
          })}
          <span style={{ width: 10 }} />
          {[7, 8].map((r) => {
            const on = filter.min_rating === r;
            return (
              <button
                key={r}
                className={"btn ghost" + (on ? " on" : "")}
                style={{ fontSize: 11.5, padding: "2px 10px", color: on ? "var(--accent)" : "var(--text-dim)", border: "1px solid var(--border)", borderRadius: 999 }}
                onClick={() => setUi({ vodFilter: { ...filter, min_rating: on ? null : r } })}
                title={`Rating ${r} or higher`}
              >
                ★ {r}+
              </button>
            );
          })}
          {(filter.year_from != null || filter.min_rating != null) && (
            <button className="btn ghost" style={{ fontSize: 11.5, padding: "2px 8px", color: "var(--text-faint)" }} onClick={() => setUi({ vodFilter: {} })}>
              Clear
            </button>
          )}
        </div>
      )}

      {menu && (
        <div className="fixed z-50 panel rounded-md py-1" style={{ left: menu.x, top: menu.y, minWidth: 200 }} onClick={(e) => e.stopPropagation()}>
          <div className="px-3 py-1 truncate" style={{ color: "var(--text-faint)", fontSize: 11.5 }}>
            {menu.v.title}
          </div>
          {[
            { icon: menu.v.kind === "series" ? ("list" as const) : ("play" as const), label: menu.v.kind === "series" ? "Episodes…" : "Play", run: () => open(menu.v) },
            ...(menu.v.kind === "movie" ? [{ icon: "play" as const, label: "Play from start", run: () => void playVod(menu.v, true) }] : []),
            ...(menu.v.kind === "movie"
              ? [
                  {
                    icon: "download" as const,
                    label: "Download (PRO)",
                    run: () =>
                      void ipc
                        .downloadItem("vod", menu.v.id)
                        .then((d) => pushToast({ level: "info", title: "Download queued", body: d.path }))
                        .catch((e) => pushToast({ level: "error", title: "Download refused", body: String(e) })),
                  },
                ]
              : []),
            { icon: "undo" as const, label: "Clear progress", run: () => void ipc.clearProgress("vod", menu.v.id).then(() => useApp.setState((s) => ({ listVersion: s.listVersion + 1 }))) },
          ].map((m) => (
            <div key={m.label} className="px-3 py-1.5 cursor-default flex items-center gap-2" style={{ fontSize: 12.5 }} onClick={() => { m.run(); setMenu(null); }} onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-elev-2)")} onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
              <Icon name={m.icon} size={14} style={{ color: "var(--text-faint)" }} />
              {m.label}
            </div>
          ))}
        </div>
      )}
      {showContinue ? (
        <div ref={parentRef} className="flex-1 min-h-0 overflow-auto p-2">
          <div className="grid gap-1" style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }}>
            {continueItems.map((c) => (
              <ContinueCard key={`${c.progress.item_type}:${c.progress.item_id}`} item={c} />
            ))}
            {continueItems.length === 0 && (
              <div className="p-6" style={{ color: "var(--text-faint)", gridColumn: "1 / -1" }}>
                Nothing in progress. Start a movie or an episode and it will show up here.
              </div>
            )}
          </div>
        </div>
      ) : (
        <div ref={parentRef} className="flex-1 min-h-0 overflow-auto px-2">
          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {items.map((r) => (
              <div key={r.key} style={{ position: "absolute", top: 0, left: 0, right: 0, height: r.size, transform: `translateY(${r.start}px)` }} className="grid gap-1" data-cols={cols}>
                <div className="grid gap-1" style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }}>
                  {Array.from({ length: cols }).map((_, c) => {
                    const idx = r.index * cols + c;
                    if (idx >= src.count) return <div key={c} />;
                    const v = src.row(idx);
                    return v ? (
                      <Poster
                        key={v.id}
                        v={v}
                        selected={idx === selectedIndex}
                        pct={progress.get(`vod:${v.id}`)}
                        onOpen={() => {
                          setUi({ selectedIndex: idx });
                          open(v);
                        }}
                        onMenu={(x, y) => setMenu({ x, y, v })}
                      />
                    ) : (
                      <div key={c} className="poster">
                        <div className="art" />
                        <div className="title" style={{ color: "var(--text-faint)" }}>
                          …
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
          {src.count === 0 && !src.loading && (
            <div className="p-6 text-center" style={{ color: "var(--text-faint)" }}>
              {search.trim() ? "No matches." : `No ${kind === "movie" ? "movies" : "series"} yet — Xtream playlists sync their library after the channels.`}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function Poster({ v, selected, pct, onOpen, onMenu }: { v: VodRecord; selected: boolean; pct?: number; onOpen: () => void; onMenu: (x: number, y: number) => void }) {
  const [ok, setOk] = useState(!!v.poster);
  return (
    <div
      className={"poster" + (selected ? " selected" : "")}
      onClick={onOpen}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e.clientX, e.clientY);
      }}
      title={v.description ?? undefined}
    >
      <div className="art-wrap">
        {ok && v.poster ? <img className="art" src={v.poster} loading="lazy" alt="" draggable={false} onError={() => setOk(false)} /> : <div className="art flex items-center justify-center" style={{ color: "var(--text-faint)", fontSize: 11.5, padding: 8, textAlign: "center" }}>{v.title}</div>}
        {pct != null && pct > 0 && (
          <div className="prog">
            <div style={{ width: `${Math.min(100, pct * 100)}%` }} />
          </div>
        )}
      </div>
      <div className="title">{v.title}</div>
      <div className="meta">
        {[v.year, v.rating ? `${v.rating.toFixed(1)} ★` : null, v.duration_s ? fmtDuration(v.duration_s) : null, v.kind === "series" ? "series" : null].filter(Boolean).join(" · ")}
      </div>
    </div>
  );
}

function ContinueCard({ item }: { item: ContinueItem }) {
  const playVod = useApp((s) => s.playVod);
  const playEpisode = useApp((s) => s.playEpisode);
  const p = item.progress;
  const d = p.duration_s ?? item.vod?.duration_s ?? item.episode?.duration ?? 0;
  const pct = d > 0 ? p.position_s / d : 0;
  const poster = item.vod?.poster ?? item.episode?.poster ?? item.series?.poster ?? null;
  const title = item.vod?.title ?? `${item.series?.title ?? "Series"} · S${item.episode?.season}E${item.episode?.episode}`;
  return (
    <div
      className="poster"
      onClick={() => {
        if (item.vod) void playVod(item.vod);
        else if (item.episode) void playEpisode(item.episode, item.series);
      }}
    >
      <div className="art-wrap">
        {poster ? <img className="art" src={poster} loading="lazy" alt="" draggable={false} /> : <div className="art" />}
        <div className="prog">
          <div style={{ width: `${Math.min(100, pct * 100)}%` }} />
        </div>
      </div>
      <div className="title">{title}</div>
      <div className="meta">
        {fmtDuration(p.position_s)}
        {d > 0 && ` / ${fmtDuration(d)}`}
      </div>
    </div>
  );
}
