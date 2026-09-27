import { useApp } from "../../lib/store";

/** `1920x1080 · H.264 · 3.3 Mbps · 50fps · cache 2.1s · zap 640ms` (CLAUDE.md §8). */
export default function Hud() {
  const t = useApp((s) => s.telemetry);
  const zap = useApp((s) => s.lastZapMs);
  const playback = useApp((s) => s.playback);
  if (!t) return <div className="hud">connecting…</div>;
  const parts: string[] = [];
  if (t.width && t.height) parts.push(`${t.width}x${t.height}`);
  if (t.codec_name) parts.push(shortCodec(t.codec_name));
  if (t.bitrate_kbps) parts.push(`${(t.bitrate_kbps / 1000).toFixed(1)} Mbps`);
  if (t.fps) parts.push(`${Math.round(t.fps)}fps`);
  parts.push(`cache ${t.cache_duration_secs.toFixed(1)}s`);
  if (t.dropped_frames) parts.push(`drop ${t.dropped_frames}`);
  if (zap != null) parts.push(`zap ${zap}ms`);
  parts.push(playback?.profile === "low_latency" ? "LL" : "ST");
  return <div className="hud">{parts.join(" · ")}</div>;
}

export function shortCodec(name: string): string {
  const n = name.toLowerCase();
  if (n.includes("hevc") || n.includes("h.265") || n.includes("265")) return "HEVC";
  if (n.includes("h.264") || n.includes("avc") || n.includes("264")) return "H.264";
  if (n.includes("av1")) return "AV1";
  if (n.includes("mpeg-2") || n.includes("mpeg2")) return "MPEG-2";
  if (n.includes("vp9")) return "VP9";
  return name.split("/")[0].trim().slice(0, 12);
}
