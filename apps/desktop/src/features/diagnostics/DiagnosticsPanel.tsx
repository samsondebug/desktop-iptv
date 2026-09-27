/**
 * Diagnostics (CLAUDE.md §9) — day-1 slice: engine log (redacted), telemetry, raw URL probe via
 * the engine, and a one-click sanitized report. HTTP tracing + isolated probe land in days 71–90.
 */
import { useState } from "react";
import { ipc } from "../../lib/ipc";
import { useApp } from "../../lib/store";

export default function DiagnosticsPanel() {
  const boot = useApp((s) => s.boot)!;
  const setUi = useApp((s) => s.setUi);
  const log = useApp((s) => s.engineLog);
  const t = useApp((s) => s.telemetry);
  const playback = useApp((s) => s.playback);
  const current = useApp((s) => s.currentChannel);
  const zap = useApp((s) => s.lastZapMs);
  const pushToast = useApp((s) => s.pushToast);
  const [url, setUrl] = useState("");
  const [copied, setCopied] = useState(false);
  const close = () => setUi({ diagnosticsOpen: false });

  const report = () =>
    [
      `${boot.product} ${boot.version} (${boot.platform})`,
      `engine: ${boot.engine_description}`,
      `profile: ${playback?.profile ?? "-"}  zap: ${zap ?? "-"} ms`,
      `channel: ${current ? `${current.name} [${current.group_title ?? "-"}]` : "-"}`,
      `stream: ${playback?.stream_url_redacted ?? "-"}`,
      `telemetry: ${t ? JSON.stringify(t) : "-"}`,
      "",
      "engine log (redacted):",
      ...log.map((l) => `${new Date(l.at).toISOString()} [${l.level}] ${l.text}`),
    ].join("\n");

  const copy = async () => {
    await navigator.clipboard.writeText(report());
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const probe = async () => {
    try {
      await ipc.loadStream({ stream_url: url.trim(), profile_mode: playback?.profile ?? "stable", audio_boost: playback?.volume ?? 100 });
      useApp.setState({ currentChannel: { id: -1, playlist_id: -1, source_id: "probe", name: "URL probe", normalized_name: "", group_title: null, logo: null, stream_url: "", tvg_id: null, catchup_days: 0 } });
    } catch (e) {
      pushToast({ level: "error", title: "Probe failed", body: String(e) });
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: "min(820px, 94vw)" }}>
        <div className="flex items-center justify-between mb-3">
          <div className="text-[16px] font-semibold">Diagnostics</div>
          <div className="flex gap-2">
            <button className="btn" onClick={() => void copy()}>
              {copied ? "Copied" : "Copy sanitized report"}
            </button>
            <button className="btn ghost" onClick={close}>
              ✕
            </button>
          </div>
        </div>

        <div className="rounded-md p-3 mb-3" style={{ background: "var(--bg-elev-2)", fontSize: 12, lineHeight: 1.6 }}>
          <b>Isolate in this order, changing one variable per test:</b> another channel on the same source → this stream ·
          another source works → this provider · Ethernet / VPN off changes it → the path · stop recordings / multiscreen → local load.
        </div>

        <div className="field mb-3">
          <label>Open a raw stream URL in the engine</label>
          <div className="flex gap-2">
            <input className="input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://…/live/user/pass/123.ts" spellCheck={false} />
            <button className="btn" disabled={!/^https?:\/\//i.test(url.trim())} onClick={() => void probe()}>
              Play
            </button>
          </div>
        </div>

        <div className="field">
          <label>Engine log (warn+, secrets redacted)</label>
          <pre
            className="rounded-md p-2 overflow-auto"
            style={{ background: "var(--bg)", border: "1px solid var(--border)", maxHeight: 280, fontSize: 11, fontFamily: "var(--mono)", color: "var(--text-dim)" }}
          >
            {log.length === 0 ? "— nothing logged yet —" : log.map((l) => `${new Date(l.at).toLocaleTimeString()} [${l.level}] ${l.text}`).join("\n")}
          </pre>
        </div>
      </div>
    </div>
  );
}
