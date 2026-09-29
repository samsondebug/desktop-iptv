/**
 * Virtualized Live list (CLAUDE.md §15 step 4). 20k rows at 60 fps is the exit test: only the
 * visible rows exist in the DOM, and rows are paged from SQLite via `useChannelSource`.
 */
import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { LIST_ENTER, LIST_MOVE } from "../../lib/keys";
import { useApp } from "../../lib/store";
import { type ChannelRecord } from "../../lib/ipc";
import { useChannelSource } from "./useChannelSource";
import Icon from "../../components/Icon";

const ROW_H = 44;

export default function ChannelList() {
  const src = useChannelSource();
  const rail = useApp((s) => s.rail);
  const search = useApp((s) => s.search);
  const setSearch = useApp((s) => s.setSearch);
  const play = useApp((s) => s.play);
  const current = useApp((s) => s.currentChannel);
  const favoriteIds = useApp((s) => s.favoriteIds);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const hideChannel = useApp((s) => s.hideChannel);
  const hideGroup = useApp((s) => s.hideGroup);
  const selectedIndex = useApp((s) => s.ui.selectedIndex);
  const setUi = useApp((s) => s.setUi);
  const parentRef = useRef<HTMLDivElement>(null);
  const [fps, setFps] = useState<number | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; ch: ChannelRecord } | null>(null);

  // Close the context menu on any click elsewhere or Escape.
  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setMenu(null);
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey);
    };
  }, [menu]);

  const virtualizer = useVirtualizer({
    count: src.count,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 12,
  });
  const items = virtualizer.getVirtualItems();

  // Prefetch the pages the viewport (plus overscan) touches.
  useEffect(() => {
    if (items.length) src.ensure(items[0].index, items[items.length - 1].index);
  }, [items, src]);

  // Keyboard navigation from the global handler.
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

  // Scroll FPS meter while scrolling (dev aid for the 60 fps exit test; hidden when idle).
  useEffect(() => {
    const el = parentRef.current;
    if (!el) return;
    let frames = 0;
    let raf = 0;
    let last = performance.now();
    let idle: number | undefined;
    const tick = () => {
      frames++;
      const now = performance.now();
      if (now - last >= 500) {
        setFps(Math.round((frames * 1000) / (now - last)));
        frames = 0;
        last = now;
      }
      raf = requestAnimationFrame(tick);
    };
    const onScroll = () => {
      if (!raf) {
        last = performance.now();
        frames = 0;
        raf = requestAnimationFrame(tick);
      }
      window.clearTimeout(idle);
      idle = window.setTimeout(() => {
        cancelAnimationFrame(raf);
        raf = 0;
        setFps(null);
      }, 700);
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      el.removeEventListener("scroll", onScroll);
      cancelAnimationFrame(raf);
      window.clearTimeout(idle);
    };
  }, []);

  const title =
    search.trim() ? `Search: “${search.trim()}”` : rail.kind === "group" ? rail.title || "(no group)" : rail.kind === "favorites" ? "Favorites" : rail.kind === "recents" ? "Recently viewed" : "All channels";

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-2 px-3 py-1.5 border-b shrink-0" style={{ borderColor: "var(--border)" }}>
        <div className="font-semibold truncate">{title}</div>
        <div style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11.5 }}>
          {src.loading ? "…" : src.count.toLocaleString()}
          {fps != null && ` · ${fps} fps`}
        </div>
        <div className="flex-1" />
        <input
          id="channel-search"
          className="input"
          style={{ width: 320 }}
          placeholder="Search channels and groups  ( / )"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          spellCheck={false}
          autoComplete="off"
        />
      </div>
      <div ref={parentRef} className="flex-1 min-h-0 overflow-auto">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {items.map((v) => {
            const ch = src.row(v.index);
            return (
              <div
                key={v.key}
                style={{ position: "absolute", top: 0, left: 0, right: 0, height: v.size, transform: `translateY(${v.start}px)` }}
              >
                {ch ? (
                  <Row
                    ch={ch}
                    index={v.index}
                    selected={v.index === selectedIndex}
                    active={current?.id === ch.id}
                    fav={favoriteIds.has(ch.id)}
                    showGroup={rail.kind !== "group"}
                    onPlay={() => {
                      setUi({ selectedIndex: v.index });
                      void play(ch);
                    }}
                    onFav={() => void toggleFavorite(ch)}
                    onMenu={(x, y) => setMenu({ x, y, ch })}
                  />
                ) : (
                  <div className="row" style={{ color: "var(--text-faint)" }}>
                    <span className="idx">{v.index + 1}</span>
                    <span />
                    <span className="name">…</span>
                  </div>
                )}
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

      {menu && (
        <div className="fixed z-50 panel rounded-md py-1" style={{ left: menu.x, top: menu.y, minWidth: 210 }} onMouseDown={(e) => e.stopPropagation()}>
          <div className="px-3 py-1 truncate" style={{ color: "var(--text-faint)", fontSize: 11.5 }}>
            {menu.ch.name}
          </div>
          <MenuRow icon="play" label="Play" onClick={() => { void play(menu.ch); setMenu(null); }} />
          <MenuRow icon={favoriteIds.has(menu.ch.id) ? "starFilled" : "star"} label={favoriteIds.has(menu.ch.id) ? "Remove favorite" : "Add favorite"} onClick={() => { void toggleFavorite(menu.ch); setMenu(null); }} />
          <MenuRow icon="edit" label="Rename channel…" onClick={() => { setUi({ renameChannel: menu.ch }); setMenu(null); }} />
          <MenuRow icon="eyeOff" label="Hide channel" onClick={() => { void hideChannel(menu.ch); setMenu(null); }} />
          {menu.ch.group_title && (
            <MenuRow icon="eyeOff" label={`Hide group “${menu.ch.group_title.slice(0, 24)}${menu.ch.group_title.length > 24 ? "…" : ""}”`} onClick={() => { void hideGroup(menu.ch.playlist_id, menu.ch.group_title!); setMenu(null); }} />
          )}
          <MenuRow icon="record" label="Record now…" onClick={() => { window.dispatchEvent(new CustomEvent("diptv:record", { detail: menu.ch })); setMenu(null); }} />
        </div>
      )}
    </div>
  );
}

function MenuRow({ icon, label, onClick }: { icon: Parameters<typeof Icon>[0]["name"]; label: string; onClick: () => void }) {
  return (
    <div className="px-3 py-1.5 cursor-default flex items-center gap-2" style={{ fontSize: 12.5 }} onClick={onClick} onMouseEnter={(e) => (e.currentTarget.style.background = "var(--bg-elev-2)")} onMouseLeave={(e) => (e.currentTarget.style.background = "transparent")}>
      <Icon name={icon} size={14} style={{ color: "var(--text-faint)" }} />
      {label}
    </div>
  );
}

function Row({
  ch,
  index,
  selected,
  active,
  fav,
  showGroup,
  onPlay,
  onFav,
  onMenu,
}: {
  ch: ChannelRecord;
  index: number;
  selected: boolean;
  active: boolean;
  fav: boolean;
  showGroup: boolean;
  onPlay: () => void;
  onFav: () => void;
  onMenu: (x: number, y: number) => void;
}) {
  const [logoOk, setLogoOk] = useState(!!ch.logo);
  return (
    <div
      className={"row" + (selected ? " selected" : "") + (active ? " active" : "")}
      onClick={onPlay}
      onDoubleClick={onPlay}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(e.clientX, e.clientY);
      }}
    >
      <span className="idx">{index + 1}</span>
      {logoOk && ch.logo ? (
        <img className="logo" src={ch.logo} loading="lazy" alt="" onError={() => setLogoOk(false)} draggable={false} />
      ) : (
        <span className="logo" />
      )}
      <span className="min-w-0">
        <span className="name block">{ch.name}</span>
        {showGroup && ch.group_title && <span className="grp block">{ch.group_title}</span>}
      </span>
      <span className="flex items-center gap-2">
        {ch.catchup_days > 0 && (
          <span className="kbd" title={`Catch-up ${ch.catchup_days} days`}>
            <Icon name="clock" size={11} /> {ch.catchup_days}d
          </span>
        )}
        {active && <span className="badge-live">LIVE</span>}
        <button
          className="btn ghost"
          style={{ color: fav ? "var(--warn)" : "var(--text-faint)", padding: "0 6px" }}
          onClick={(e) => {
            e.stopPropagation();
            onFav();
          }}
          title="Favorite"
        >
          <Icon name={fav ? "starFilled" : "star"} size={15} />
        </button>
      </span>
    </div>
  );
}
