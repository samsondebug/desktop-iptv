/**
 * The EPG grid (CLAUDE.md §8): channel rows × horizontal time axis, now-line, 8 h window,
 * hover description, click to play, right-click for Edit EPG / favorite / record.
 * Rows are the same virtualized, paged channel source as the plain list, so a 20k-channel
 * guide costs the same as a 20k-channel list; programmes are fetched only for visible rows.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ipc, type ChannelRecord, type Programme } from "../../lib/ipc";
import { EPG_SHIFT, LIST_ENTER, LIST_MOVE } from "../../lib/keys";
import { fmtTime, useApp } from "../../lib/store";
import { useChannelSource } from "../live/useChannelSource";

const ROW_H = 52;
const CH_COL_W = 220;
const WINDOW_HOURS_OPTIONS = [4, 8, 12];

interface Hover {
  x: number;
  y: number;
  p: Programme;
  ch: ChannelRecord;
}

interface Menu {
  x: number;
  y: number;
  ch: ChannelRecord;
  p?: Programme;
}

function floorToHalfHour(unix: number) {
  return Math.floor(unix / 1800) * 1800;
}

export default function EpgGrid() {
  const src = useChannelSource();
  const playlistId = useApp((s) => s.activePlaylistId);
  const epgVersion = useApp((s) => s.epgVersion);
  const epgStats = useApp((s) => s.epgStats);
  const imports = useApp((s) => s.imports);
  const play = useApp((s) => s.play);
  const current = useApp((s) => s.currentChannel);
  const favoriteIds = useApp((s) => s.favoriteIds);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const selectedIndex = useApp((s) => s.ui.selectedIndex);
  const setUi = useApp((s) => s.setUi);
  const search = useApp((s) => s.search);
  const setSearch = useApp((s) => s.setSearch);
  const rail = useApp((s) => s.rail);
  const guideMode = useApp((s) => s.guideMode);

  const [hours, setHours] = useState(8);
  const [windowStart, setWindowStart] = useState(() => floorToHalfHour(Date.now() / 1000) - 1800);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  const [hover, setHover] = useState<Hover | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [gridW, setGridW] = useState(800);
  const [, bump] = useState(0);
  const parentRef = useRef<HTMLDivElement>(null);
  const cache = useRef(new Map<number, Programme[]>());
  const cacheKey = useRef("");
  const inflight = useRef(new Set<number>());

  const windowEnd = windowStart + hours * 3600;
  const pxPerSec = Math.max(0.01, (gridW - CH_COL_W) / (hours * 3600));

  // Clock + now-line.
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  }, []);

  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setGridW(el.clientWidth));
    ro.observe(el);
    setGridW(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  // Window navigation from the keyboard (←/→) and buttons.
  const shift = useCallback((h: number) => setWindowStart((s) => s + h * 3600), []);
  const jumpNow = useCallback(() => setWindowStart(floorToHalfHour(Date.now() / 1000) - 1800), []);
  useEffect(() => {
    const on = (e: Event) => shift((e as CustomEvent<number>).detail);
    window.addEventListener(EPG_SHIFT, on);
    return () => window.removeEventListener(EPG_SHIFT, on);
  }, [shift]);

  const virtualizer = useVirtualizer({
    count: src.count,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 8,
  });
  const items = virtualizer.getVirtualItems();

  // Invalidate the programme cache when the window / playlist / guide changes.
  const key = `${playlistId}|${windowStart}|${hours}|${epgVersion}`;
  if (cacheKey.current !== key) {
    cacheKey.current = key;
    cache.current.clear();
    inflight.current.clear();
  }

  // Fetch programmes for the visible rows (debounced).
  useEffect(() => {
    if (items.length) src.ensure(items[0].index, items[items.length - 1].index);
    if (playlistId == null) return;
    const t = window.setTimeout(() => {
      const ids: number[] = [];
      for (const v of items) {
        const ch = src.row(v.index);
        if (ch && !cache.current.has(ch.id) && !inflight.current.has(ch.id)) ids.push(ch.id);
      }
      if (ids.length === 0) return;
      ids.forEach((id) => inflight.current.add(id));
      const myKey = cacheKey.current;
      ipc
        .epgGrid({ playlist_id: playlistId, channel_ids: ids, from: windowStart, to: windowEnd })
        .then((rows) => {
          if (myKey !== cacheKey.current) return;
          for (const r of rows) cache.current.set(r.channel_id, r.programmes);
          bump((x) => x + 1);
        })
        .catch(() => {})
        .finally(() => ids.forEach((id) => inflight.current.delete(id)));
    }, 80);
    return () => window.clearTimeout(t);
  }, [items, src, playlistId, windowStart, windowEnd, epgVersion, key]);

  // Keyboard navigation shared with the list.
  useEffect(() => {
    const onMove = (e: Event) => {
      const delta = (e as CustomEvent<number>).detail;
      const next = Math.max(0, Math.min(src.count - 1, (selectedIndex < 0 ? -1 : selectedIndex) + delta));
      setUi({ selectedIndex: next });
      virtualizer.scrollToIndex(next, { align: "auto" });
    };
    const onEnter = () => {
      if (selectedIndex < 0) return;
      void src.fetchRow(selectedIndex).then((ch) => ch && play(ch));
    };
    window.addEventListener(LIST_MOVE, onMove);
    window.addEventListener(LIST_ENTER, onEnter);
    return () => {
      window.removeEventListener(LIST_MOVE, onMove);
      window.removeEventListener(LIST_ENTER, onEnter);
    };
  }, [selectedIndex, src, setUi, virtualizer, play]);

  useEffect(() => {
    const close = () => setMenu(null);
    window.addEventListener("click", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("click", close);
      window.removeEventListener("scroll", close, true);
    };
  }, []);

  const ticks = useMemo(() => {
    const out: number[] = [];
    for (let t = windowStart; t < windowEnd; t += 1800) out.push(t);
    return out;
  }, [windowStart, windowEnd]);

  const importing = playlistId != null ? imports[playlistId] : undefined;
  const guideImporting = importing?.phase === "epg";
  const title = search.trim()
    ? `Search: “${search.trim()}”`
    : rail.kind === "group"
      ? rail.title || "(no group)"
      : rail.kind === "favorites"
        ? "Favorites"
        : rail.kind === "recents"
          ? "Recently viewed"
          : "All channels";

  return (
    <div className="h-full flex flex-col" onMouseLeave={() => setHover(null)}>
      <div className="flex items-center gap-2 px-3 py-1.5 border-b shrink-0" style={{ borderColor: "var(--border)" }}>
        <div className="font-semibold truncate">{title}</div>
        <div style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11 }}>
          {src.loading ? "…" : src.count.toLocaleString()}
          {epgStats && epgStats.programmes > 0 && ` · guide ${epgStats.channels_with_epg.toLocaleString()} ch`}
          {guideImporting && ` · Guide importing ${importing?.message ?? ""} (${importing?.channels.toLocaleString()} programmes)`}
        </div>
        <div className="flex-1" />
        <div className="seg" title="List or guide view">
          <button className={guideMode === "list" ? "on" : ""} onClick={() => useApp.setState({ guideMode: "list" })}>
            List
          </button>
          <button className={guideMode === "guide" ? "on" : ""} onClick={() => useApp.setState({ guideMode: "guide" })}>
            Guide
          </button>
        </div>
        <button className="btn ghost" onClick={() => shift(-2)} title="Earlier (←)">
          ◀
        </button>
        <button className="btn ghost" onClick={jumpNow} title="Jump to now">
          Now
        </button>
        <button className="btn ghost" onClick={() => shift(2)} title="Later (→)">
          ▶
        </button>
        <select className="input" style={{ width: 72 }} value={hours} onChange={(e) => setHours(Number(e.target.value))} title="Window">
          {WINDOW_HOURS_OPTIONS.map((h) => (
            <option key={h} value={h}>
              {h} h
            </option>
          ))}
        </select>
        <input
          id="channel-search"
          className="input"
          style={{ width: 260 }}
          placeholder="Search channels and groups  ( / )"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          spellCheck={false}
          autoComplete="off"
        />
      </div>

      {/* time axis */}
      <div className="relative shrink-0 border-b" style={{ height: 24, borderColor: "var(--border)", background: "var(--bg-elev)" }}>
        <div className="absolute left-0 top-0 bottom-0 flex items-center px-3" style={{ width: CH_COL_W, color: "var(--text-faint)", fontSize: 11 }}>
          {new Date(windowStart * 1000).toLocaleDateString([], { weekday: "short", month: "short", day: "numeric" })}
        </div>
        {ticks.map((t) => (
          <div
            key={t}
            className="absolute top-0 bottom-0 border-l"
            style={{ left: CH_COL_W + (t - windowStart) * pxPerSec, borderColor: "var(--border)", color: "var(--text-dim)", fontSize: 11, paddingLeft: 4, lineHeight: "24px", fontFamily: "var(--mono)" }}
          >
            {fmtTime(t)}
          </div>
        ))}
        {now >= windowStart && now <= windowEnd && (
          <div className="absolute top-0 bottom-0" style={{ left: CH_COL_W + (now - windowStart) * pxPerSec, width: 2, background: "var(--live)" }} />
        )}
      </div>

      <div ref={parentRef} className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden relative">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {items.map((v) => {
            const ch = src.row(v.index);
            const progs = ch ? cache.current.get(ch.id) : undefined;
            const isActive = !!ch && current?.id === ch.id;
            const isSel = v.index === selectedIndex;
            return (
              <div
                key={v.key}
                className={"epg-row" + (isSel ? " selected" : "") + (isActive ? " active" : "")}
                style={{ position: "absolute", top: 0, left: 0, right: 0, height: v.size, transform: `translateY(${v.start}px)` }}
                onClick={() => {
                  if (!ch) return;
                  setUi({ selectedIndex: v.index });
                  void play(ch);
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  if (ch) setMenu({ x: e.clientX, y: e.clientY, ch, p: hover?.p });
                }}
              >
                <div className="epg-ch" style={{ width: CH_COL_W }}>
                  <span className="idx">{v.index + 1}</span>
                  {ch?.logo ? <img className="logo" src={ch.logo} loading="lazy" alt="" draggable={false} onError={(e) => ((e.target as HTMLImageElement).style.visibility = "hidden")} /> : <span className="logo" />}
                  <span className="min-w-0">
                    <span className="name block">{ch?.name ?? "…"}</span>
                    {ch && rail.kind !== "group" && ch.group_title && <span className="grp block">{ch.group_title}</span>}
                  </span>
                  {isActive && <span className="badge-live">LIVE</span>}
                </div>
                <div className="epg-lane" style={{ left: CH_COL_W }}>
                  {ch && progs === undefined && <span className="epg-empty">…</span>}
                  {ch && progs && progs.length === 0 && <span className="epg-empty">{ch.tvg_id ? "No guide data for this window" : "No tvg-id — right-click → Edit EPG"}</span>}
                  {progs?.map((p) => {
                    const s = Math.max(p.start, windowStart);
                    const e = Math.min(p.stop, windowEnd);
                    if (e <= s) return null;
                    const left = (s - windowStart) * pxPerSec;
                    const width = Math.max(2, (e - s) * pxPerSec - 2);
                    const live = p.start <= now && p.stop > now;
                    return (
                      <div
                        key={p.start}
                        className={"epg-prog" + (live ? " live" : "") + (width < 70 ? " narrow" : "")}
                        style={{ left, width }}
                        onMouseEnter={(ev) => ch && setHover({ x: ev.clientX, y: ev.clientY, p, ch })}
                        onMouseMove={(ev) => setHover((h) => (h ? { ...h, x: ev.clientX, y: ev.clientY } : h))}
                        onMouseLeave={() => setHover(null)}
                      >
                        <span className="epg-prog-title">{p.title}</span>
                        <span className="epg-prog-time">{fmtTime(p.start)}</span>
                      </div>
                    );
                  })}
                  {now >= windowStart && now <= windowEnd && <div className="epg-nowline" style={{ left: (now - windowStart) * pxPerSec }} />}
                </div>
              </div>
            );
          })}
        </div>
        {src.count === 0 && !src.loading && (
          <div className="p-6 text-center" style={{ color: "var(--text-faint)" }}>
            {search.trim() ? "No matches." : "Nothing here yet."}
          </div>
        )}
      </div>

      {hover && (
        <div
          className="fixed z-40 pointer-events-none"
          style={{ left: Math.min(hover.x + 14, window.innerWidth - 340), top: Math.max(8, hover.y - 10), width: 320 }}
        >
          <div className="panel rounded-md p-3" style={{ boxShadow: "0 10px 30px rgba(0,0,0,.5)" }}>
            <div className="font-semibold">{hover.p.title}</div>
            <div style={{ color: "var(--text-dim)", fontSize: 12 }}>
              {hover.ch.name} · {fmtTime(hover.p.start)}–{fmtTime(hover.p.stop)}
            </div>
            {hover.p.desc && (
              <div className="mt-1" style={{ color: "var(--text-dim)", fontSize: 12, maxHeight: 120, overflow: "hidden" }}>
                {hover.p.desc}
              </div>
            )}
          </div>
        </div>
      )}

      {menu && (
        <div className="fixed z-50 panel rounded-md py-1" style={{ left: menu.x, top: menu.y, minWidth: 200 }} onClick={(e) => e.stopPropagation()}>
          <div className="px-3 py-1 truncate" style={{ color: "var(--text-faint)", fontSize: 11 }}>
            {menu.ch.name}
          </div>
          <MenuItem label="▶ Play" onClick={() => { void play(menu.ch); setMenu(null); }} />
          <MenuItem label={favoriteIds.has(menu.ch.id) ? "★ Remove favorite" : "☆ Add favorite"} onClick={() => { void toggleFavorite(menu.ch); setMenu(null); }} />
          <MenuItem label="✎ Edit EPG (tvg-id)" onClick={() => { setUi({ epgEditChannel: menu.ch }); setMenu(null); }} />
          {menu.p && (
            <MenuItem
              label={`● Record “${menu.p.title.slice(0, 28)}${menu.p.title.length > 28 ? "…" : ""}”`}
              onClick={() => {
                window.dispatchEvent(new CustomEvent("diptv:record", { detail: { channel: menu.ch, programme: { start: menu.p!.start, stop: menu.p!.stop, title: menu.p!.title } } }));
                setMenu(null);
              }}
            />
          )}
          <MenuItem label="● Record now…" onClick={() => { window.dispatchEvent(new CustomEvent("diptv:record", { detail: menu.ch })); setMenu(null); }} />
        </div>
      )}
    </div>
  );
}

function MenuItem({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <div className="px-3 py-1.5 cursor-default" style={{ fontSize: 12.5 }} onClick={onClick} onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-elev-2)")} onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
      {label}
    </div>
  );
}
