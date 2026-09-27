/** Paged, cached window over vod_items — same shape as useChannelSource. */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ipc, type VodKind, type VodRecord } from "../../lib/ipc";
import { useApp } from "../../lib/store";

export const VOD_PAGE = 120;

export interface VodSource {
  count: number;
  loading: boolean;
  row: (index: number) => VodRecord | undefined;
  ensure: (start: number, end: number) => void;
}

export function useVodSource(kind: VodKind): VodSource {
  const playlistId = useApp((s) => s.activePlaylistId);
  const category = useApp((s) => s.ui.vodCategory);
  const sort = useApp((s) => s.ui.vodSort);
  const search = useApp((s) => s.search);
  const listVersion = useApp((s) => s.listVersion);
  const [count, setCount] = useState(0);
  const [loading, setLoading] = useState(false);
  const [array, setArray] = useState<VodRecord[] | null>(null);
  const [, bump] = useState(0);
  const pages = useRef(new Map<number, VodRecord[]>());
  const inflight = useRef(new Set<number>());
  const gen = useRef(0);

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
    setLoading(true);
    if (search.trim()) {
      const q = search.trim();
      const t = window.setTimeout(() => {
        ipc
          .searchVod(playlistId, q, kind, 600)
          .then((rows) => {
            if (g !== gen.current) return;
            setArray(rows);
            setCount(rows.length);
          })
          .finally(() => g === gen.current && setLoading(false));
      }, 120);
      return () => window.clearTimeout(t);
    }
    ipc
      .countVod(playlistId, kind, category)
      .then((n) => g === gen.current && setCount(n))
      .finally(() => g === gen.current && setLoading(false));
  }, [playlistId, kind, category, sort, search, listVersion]);

  const loadPage = useCallback(
    (p: number) => {
      if (playlistId == null || pages.current.has(p) || inflight.current.has(p)) return;
      inflight.current.add(p);
      const g = gen.current;
      ipc
        .listVod({ playlist_id: playlistId, kind, category, sort, limit: VOD_PAGE, offset: p * VOD_PAGE })
        .then((rows) => {
          if (g !== gen.current) return;
          pages.current.set(p, rows);
          bump((x) => x + 1);
        })
        .finally(() => inflight.current.delete(p));
    },
    [playlistId, kind, category, sort],
  );

  const row = useCallback(
    (index: number) => {
      if (array) return array[index];
      const p = Math.floor(index / VOD_PAGE);
      const page = pages.current.get(p);
      if (!page) {
        loadPage(p);
        return undefined;
      }
      return page[index - p * VOD_PAGE];
    },
    [array, loadPage],
  );

  const ensure = useCallback(
    (start: number, end: number) => {
      if (array) return;
      for (let p = Math.max(0, Math.floor(start / VOD_PAGE)); p <= Math.floor(end / VOD_PAGE); p++) loadPage(p);
    },
    [array, loadPage],
  );

  return useMemo(() => ({ count, loading, row, ensure }), [count, loading, row, ensure]);
}
