import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ipc, type VodGroup } from "../lib/ipc";
import { useApp, vodKindOfTab, type RailSelection } from "../lib/store";

function same(a: RailSelection, b: RailSelection) {
  return a.kind === b.kind && (a.kind !== "group" || b.kind !== "group" || a.title === b.title);
}

export default function Rail() {
  const tab = useApp((s) => s.tab);
  const groups = useApp((s) => s.groups);
  const rail = useApp((s) => s.rail);
  const selectRail = useApp((s) => s.selectRail);
  const favorites = useApp((s) => s.favorites);
  const recents = useApp((s) => s.recents);
  const playlists = useApp((s) => s.playlists);
  const activePlaylistId = useApp((s) => s.activePlaylistId);
  const listVersion = useApp((s) => s.listVersion);
  const parental = useApp((s) => s.parental);
  const setUi = useApp((s) => s.setUi);
  const vodCategory = useApp((s) => s.ui.vodCategory);
  const refreshParental = useApp((s) => s.refreshParental);
  const activeRecordings = useApp((s) => s.activeRecordings);
  const panes = useApp((s) => s.panes);
  const openPane = useApp((s) => s.openPane);
  const [filter, setFilter] = useState("");
  const [vodGroups, setVodGroups] = useState<VodGroup[]>([]);
  const [vodTotal, setVodTotal] = useState(0);
  const kind = vodKindOfTab(tab);

  useEffect(() => {
    if (!kind || activePlaylistId == null) return;
    ipc.vodGroups(activePlaylistId, kind).then(setVodGroups).catch(() => setVodGroups([]));
    ipc.countVod(activePlaylistId, kind, null).then(setVodTotal).catch(() => setVodTotal(0));
  }, [kind, activePlaylistId, listVersion]);

  const total = playlists.find((p) => p.id === activePlaylistId)?.channel_count ?? 0;
  const visible = useMemo(() => {
    const f = filter.trim().toLowerCase();
    if (kind) return f ? vodGroups.filter((g) => g.category.toLowerCase().includes(f)) : vodGroups;
    return f ? groups.filter((g) => g.group_title.toLowerCase().includes(f)) : groups;
  }, [groups, vodGroups, filter, kind]);

  const parentRef = useRef<HTMLDivElement>(null);
  const rowVirtualizer = useVirtualizer({ count: visible.length, getScrollElement: () => parentRef.current, estimateSize: () => 30, overscan: 10 });

  return (
    <div className="opaque flex flex-col border-r shrink-0" style={{ width: "var(--rail-w)", borderColor: "var(--border)" }}>
      <div className="p-2 flex flex-col gap-0.5">
        {!kind ? (
          <>
            <RailItem label="★ Favorites" count={favorites.length} active={same(rail, { kind: "favorites" })} onClick={() => selectRail({ kind: "favorites" })} />
            <RailItem label="◷ Recently viewed" count={recents.length} active={same(rail, { kind: "recents" })} onClick={() => selectRail({ kind: "recents" })} />
            <RailItem label="All channels" count={total} active={same(rail, { kind: "all" })} onClick={() => selectRail({ kind: "all" })} />
            <RailItem label="● Recordings & downloads" count={activeRecordings || null} active={false} onClick={() => setUi({ libraryOpen: true })} />
            <RailItem label="⊞ Multiscreen pane" count={panes.length || null} active={false} onClick={() => void openPane(null)} muted />
          </>
        ) : (
          <>
            <RailItem label="▶ Continue watching" count={null} active={rail.kind === "continue"} onClick={() => selectRail({ kind: "continue" })} />
            <RailItem
              label={kind === "movie" ? "All movies" : "All series"}
              count={vodTotal}
              active={rail.kind !== "continue" && vodCategory == null}
              onClick={() => {
                selectRail({ kind: "all" });
                setUi({ vodCategory: null });
              }}
            />
          </>
        )}
      </div>
      <div className="px-2 pb-1">
        <input className="input" placeholder={`Filter ${visible.length} ${kind ? "categories" : "groups"}…`} value={filter} onChange={(e) => setFilter(e.target.value)} spellCheck={false} />
      </div>
      <div ref={parentRef} className="flex-1 min-h-0 overflow-auto px-2 pb-2">
        <div style={{ height: rowVirtualizer.getTotalSize(), position: "relative" }}>
          {rowVirtualizer.getVirtualItems().map((v) => {
            const g = visible[v.index];
            const title = kind ? (g as VodGroup).category : (g as { group_title: string }).group_title;
            const count = kind ? (g as VodGroup).count : (g as { channel_count: number }).channel_count;
            const active = kind ? rail.kind !== "continue" && vodCategory === title : same(rail, { kind: "group", title });
            return (
              <div
                key={v.key}
                className={"rail-item" + (active ? " active" : "")}
                style={{ position: "absolute", top: 0, left: 0, right: 0, height: v.size, transform: `translateY(${v.start}px)` }}
                onClick={() => {
                  if (kind) {
                    selectRail({ kind: "all" });
                    setUi({ vodCategory: title });
                  } else selectRail({ kind: "group", title });
                }}
                title={title || "(none)"}
              >
                <span className="truncate">{title || "(none)"}</span>
                <span className="count">{count.toLocaleString()}</span>
              </div>
            );
          })}
        </div>
      </div>
      {parental?.enabled && (
        <div className="px-2 py-1.5 border-t flex items-center justify-between" style={{ borderColor: "var(--border)", fontSize: 11.5 }}>
          <span style={{ color: parental.unlocked ? "var(--warn)" : "var(--text-faint)" }}>{parental.unlocked ? "🔓 Filter off" : "🔒 Parental filter on"}</span>
          <button
            className="btn ghost"
            style={{ padding: "0 6px" }}
            onClick={() => (parental.unlocked ? void ipc.lockParental().then(refreshParental) : setUi({ unlockOpen: true }))}
          >
            {parental.unlocked ? "Lock" : "Unlock"}
          </button>
        </div>
      )}
    </div>
  );
}

function RailItem({ label, count, active, onClick, muted }: { label: string; count: number | null; active: boolean; onClick: () => void; muted?: boolean }) {
  return (
    <div className={"rail-item" + (active ? " active" : "")} style={muted ? { color: "var(--text-faint)" } : undefined} onClick={onClick}>
      <span>{label}</span>
      {count != null && <span className="count">{count.toLocaleString()}</span>}
    </div>
  );
}
