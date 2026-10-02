/**
 * Last-known stream health for the visible channel rows ("played at 1080p H.264 an hour ago").
 * Fed by the backend from live playback telemetry and on-demand probes — reading it here never
 * touches the provider. Batched per visible id set, cached for the session.
 */
import { useEffect, useRef, useState } from "react";
import { ipc, type HealthRecord } from "../../lib/ipc";
import { useApp } from "../../lib/store";

const cache = new Map<number, HealthRecord>();
let fetched = new Set<number>();

/** Drop the session cache (after a probe or when the catalog changes). */
export function invalidateHealth(ids?: number[]) {
  if (!ids) {
    cache.clear();
    fetched = new Set();
    return;
  }
  for (const id of ids) {
    cache.delete(id);
    fetched.delete(id);
  }
}

export function useChannelHealth(ids: number[]): Map<number, HealthRecord> {
  const [, bump] = useState(0);
  const playback = useApp((s) => s.playback);
  const playingId = playback?.item.kind === "channel" ? playback.item.id : null;
  const inflight = useRef(false);

  // The playing channel's snapshot updates every ~30 s backend-side; refetch it on zap.
  useEffect(() => {
    if (playingId != null) invalidateHealth([playingId]);
  }, [playingId]);

  useEffect(() => {
    const missing = ids.filter((id) => !fetched.has(id));
    if (missing.length === 0 || inflight.current) return;
    inflight.current = true;
    for (const id of missing) fetched.add(id);
    ipc
      .channelHealth(missing)
      .then((rows) => {
        for (const r of rows) cache.set(r.channel_id, r);
        bump((x) => x + 1);
      })
      .catch(() => {
        for (const id of missing) fetched.delete(id);
      })
      .finally(() => {
        inflight.current = false;
      });
  }, [ids.join(",")]); // eslint-disable-line react-hooks/exhaustive-deps

  return cache;
}

export function healthLabel(h: HealthRecord): string {
  if (!h.ok) return "down?";
  if (!h.height) return "ok";
  if (h.height >= 2000) return "4K";
  return `${h.height}p`;
}

export function healthTitle(h: HealthRecord, now: number): string {
  const age = Math.max(0, now - h.checked_at);
  const ago = age < 90 ? "just now" : age < 5400 ? `${Math.round(age / 60)} min ago` : `${Math.round(age / 3600)} h ago`;
  const bits = [
    h.width && h.height ? `${h.width}×${h.height}` : null,
    h.codec?.toUpperCase() ?? null,
    h.bitrate_kbps ? `${(h.bitrate_kbps / 1000).toFixed(1)} Mbps` : null,
  ].filter(Boolean);
  return `${h.ok ? "Played fine" : "Failed"} ${ago}${bits.length ? " · " + bits.join(" · ") : ""} (${h.source === "probe" ? "probe" : "while watching"})`;
}
