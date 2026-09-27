import { useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useApp, type RailSelection } from "../lib/store";

function same(a: RailSelection, b: RailSelection) {
  return a.kind === b.kind && (a.kind !== "group" || b.kind !== "group" || a.title === b.title);
}

export default function Rail() {
  const groups = useApp((s) => s.groups);
  const rail = useApp((s) => s.rail);
  const selectRail = useApp((s) => s.selectRail);
  const favorites = useApp((s) => s.favorites);
  const recents = useApp((s) => s.recents);
  const playlists = useApp((s) => s.playlists);
  const activePlaylistId = useApp((s) => s.activePlaylistId);
  const [filter, setFilter] = useState("");

  const total = playlists.find((p) => p.id === activePlaylistId)?.channel_count ?? 0;
  const visibleGroups = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return f ? groups.filter((g) => g.group_title.toLowerCase().includes(f)) : groups;
  }, [groups, filter]);

  const parentRef = useRef<HTMLDivElement>(null);
  const rowVirtualizer = useVirtualizer({
    count: visibleGroups.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 30,
    overscan: 10,
  });

  const fixed: { sel: RailSelection; label: string; count: number }[] = [
    { sel: { kind: "favorites" }, label: "★ Favorites", count: favorites.length },
    { sel: { kind: "recents" }, label: "◷ Recently viewed", count: recents.length },
    { sel: { kind: "all" }, label: "All channels", count: total },
  ];

  return (
    <div
      className="opaque flex flex-col border-r shrink-0"
      style={{ width: "var(--rail-w)", borderColor: "var(--border)" }}
    >
      <div className="p-2 flex flex-col gap-0.5">
        {fixed.map((f) => (
          <div key={f.label} className={"rail-item" + (same(rail, f.sel) ? " active" : "")} onClick={() => selectRail(f.sel)}>
            <span>{f.label}</span>
            <span className="count">{f.count.toLocaleString()}</span>
          </div>
        ))}
        <div className="rail-item" style={{ color: "var(--text-faint)" }} title="Days 51–70">
          <span>● Recordings</span>
          <span className="count">PRO</span>
        </div>
      </div>
      <div className="px-2 pb-1">
        <input
          className="input"
          placeholder={`Filter ${groups.length} groups…`}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          spellCheck={false}
        />
      </div>
      <div ref={parentRef} className="flex-1 min-h-0 overflow-auto px-2 pb-2">
        <div style={{ height: rowVirtualizer.getTotalSize(), position: "relative" }}>
          {rowVirtualizer.getVirtualItems().map((v) => {
            const g = visibleGroups[v.index];
            const sel: RailSelection = { kind: "group", title: g.group_title };
            return (
              <div
                key={v.key}
                className={"rail-item" + (same(rail, sel) ? " active" : "")}
                style={{ position: "absolute", top: 0, left: 0, right: 0, height: v.size, transform: `translateY(${v.start}px)` }}
                onClick={() => selectRail(sel)}
                title={g.group_title || "(no group)"}
              >
                <span className="truncate">{g.group_title || "(no group)"}</span>
                <span className="count">{g.channel_count.toLocaleString()}</span>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
