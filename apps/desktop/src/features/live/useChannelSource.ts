/**
 * Paged, cached window over SQLite for the virtualized list. The full catalog never lives
 * in JS: only the pages the viewport touched (plus overscan) are held, and the cache is
 * dropped whenever the source (playlist / rail / search / import) changes.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ipc, type ChannelRecord } from "../../lib/ipc";
import { useApp } from "../../lib/store";

export const PAGE = 200;
const SEARCH_LIMIT = 1000;

export interface ChannelSource {
  count: number;
  loading: boolean;
  /** Row at `index` if already loaded; triggers a page fetch otherwise. */
  row: (index: number) => ChannelRecord | undefined;
  /** Prefetch pages covering [start, end]. */
  ensure: (start: number, end: number) => void;
  /** Resolve a row even if its page is not loaded yet (used by Enter / autoplay). */
  fetchRow: (index: number) => Promise<ChannelRecord | undefined>;
  mode: "paged" | "array";
}

export function useChannelSource(): ChannelSource {
  const playlistId = useApp((s) => s.activePlaylistId);
  const rail = useApp((s) => s.rail);
  const search = useApp((s) => s.search);
  const listVersion = useApp((s) => s.listVersion);
  const favorites = useApp((s) => s.favorites);
  const recents = useApp((s) => s.recents);

  const [count, setCount] = useState(0);
  const [loading, setLoading] = useState(false);
  const [array, setArray] = useState<ChannelRecord[] | null>(null);
  const [, bump] = useState(0);
  const pages = useRef(new Map<number, ChannelRecord[]>());
  const inflight = useRef(new Set<number>());
  const gen = useRef(0);

  const groupTitle = rail.kind === "group" ? rail.title : null;
  const mode: "paged" | "array" = search.trim() || rail.kind === "favorites" || rail.kind === "recents" ? "array" : "paged";

  // Reset on any source change.
  useEffect(() => {
    gen.current += 1;
    const g = gen.current;
    pages.current.clear();
    inflight.current.clear();
    setArray(null);
    if (playlistId == null) {
      setCount(0);
      return;
    }
    if (search.trim()) {
      setLoading(true);
      const q = search.trim();
      const t = window.setTimeout(() => {
        ipc
          .searchChannels({ query_string: q, playlist_id: playlistId, limit: SEARCH_LIMIT, offset: 0 })
          .then((rows) => {
            if (g !== gen.current) return;
            setArray(rows);
            setCount(rows.length);
          })
          .finally(() => g === gen.current && setLoading(false));
      }, 120); // debounce keystrokes
      return () => window.clearTimeout(t);
    }
    if (rail.kind === "favorites") {
      setArray(favorites);
      setCount(favorites.length);
      return;
    }
    if (rail.kind === "recents") {
      setArray(recents);
      setCount(recents.length);
      return;
    }
    setLoading(true);
    ipc
      .countChannels(playlistId, groupTitle)
      .then((n) => g === gen.current && setCount(n))
      .finally(() => g === gen.current && setLoading(false));
  }, [playlistId, rail.kind, groupTitle, search, listVersion, favorites, recents]);

  const loadPage = useCallback(
    (p: number) => {
      if (playlistId == null || pages.current.has(p) || inflight.current.has(p)) return;
      inflight.current.add(p);
      const g = gen.current;
      ipc
        .listChannels({ playlist_id: playlistId, group_title: groupTitle, limit: PAGE, offset: p * PAGE })
        .then((rows) => {
          if (g !== gen.current) return;
          pages.current.set(p, rows);
          bump((x) => x + 1);
        })
        .finally(() => inflight.current.delete(p));
    },
    [playlistId, groupTitle],
  );

  const row = useCallback(
    (index: number): ChannelRecord | undefined => {
      if (array) return array[index];
      const p = Math.floor(index / PAGE);
      const page = pages.current.get(p);
      if (!page) {
        loadPage(p);
        return undefined;
      }
      return page[index - p * PAGE];
    },
    [array, loadPage],
  );

  const ensure = useCallback(
    (start: number, end: number) => {
      if (array) return;
      const p0 = Math.max(0, Math.floor(start / PAGE));
      const p1 = Math.max(0, Math.floor(end / PAGE));
      for (let p = p0; p <= p1; p++) loadPage(p);
    },
    [array, loadPage],
  );

  const fetchRow = useCallback(
    async (index: number): Promise<ChannelRecord | undefined> => {
      if (array) return array[index];
      if (playlistId == null) return undefined;
      const p = Math.floor(index / PAGE);
      const cached = pages.current.get(p);
      if (cached) return cached[index - p * PAGE];
      const rows = await ipc.listChannels({ playlist_id: playlistId, group_title: groupTitle, limit: PAGE, offset: p * PAGE });
      pages.current.set(p, rows);
      bump((x) => x + 1);
      return rows[index - p * PAGE];
    },
    [array, playlistId, groupTitle],
  );

  return useMemo(() => ({ count, loading, row, ensure, fetchRow, mode }), [count, loading, row, ensure, fetchRow, mode]);
}
