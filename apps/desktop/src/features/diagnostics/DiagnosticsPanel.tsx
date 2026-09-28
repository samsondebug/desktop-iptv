/**
 * Diagnostics workbench (CLAUDE.md §9).
 *   Report   — one-click sanitized report (Rust assembles it; we append the engine log).
 *   HTTP     — every request the app made: method, redacted URL, status, timing, type, first
 *              200 bytes, redirect chain.
 *   Probe    — isolated headless libmpv probe of any URL (container, codecs, time-to-first-frame,
 *              error) without touching the player; or play the URL in the main player.
 *   Sources  — fetch + preflight + parse-count a playlist ("got HTML, not M3U"), or Xtream
 *              account status / expiry / connections (never the password).
 */
import { useEffect, useState } from "react";
import { ipc, type HttpTrace, type ProbeResult, type SourceCheck } from "../../lib/ipc";
import { useApp } from "../../lib/store";
import Icon from "../../components/Icon";

type Tab = "report" | "http" | "probe" | "sources";

export default function DiagnosticsPanel() {
  const boot = useApp((s) => s.boot)!;
  const setUi = useApp((s) => s.setUi);
  const log = useApp((s) => s.engineLog);
  const playback = useApp((s) => s.playback);
  const pushToast = useApp((s) => s.pushToast);
  const [tab, setTab] = useState<Tab>("report");
  const [copied, setCopied] = useState(false);
  const close = () => setUi({ diagnosticsOpen: false });

  const buildReport = async () => {
    const head = await ipc.diagReport();
    const tail = ["", "engine log (warn+, redacted):", ...log.map((l) => `${new Date(l.at).toISOString()} [${l.level}] ${l.text}`)].join("\n");
    return head + tail;
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(await buildReport());
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      pushToast({ level: "error", title: "Copy failed", body: String(e) });
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal flex flex-col" style={{ width: "min(960px, 96vw)", height: "min(720px, 92vh)" }}>
        <div className="flex items-center justify-between mb-2">
          <div>
            <div className="text-[16px] font-semibold">Diagnostics</div>
            <div style={{ color: "var(--text-faint)", fontSize: 11.5 }}>
              {boot.product} {boot.version} · {boot.platform} · engine {boot.engine_kind}
            </div>
          </div>
          <div className="flex gap-2">
            <button className="btn primary" onClick={() => void copy()}>
              {copied ? "Copied" : "Copy sanitized report"}
            </button>
            <button className="btn ghost" onClick={close}>
              <Icon name="close" size={16} />
            </button>
          </div>
        </div>
        <div className="tabs">
          {(["report", "http", "probe", "sources"] as Tab[]).map((t) => (
            <button key={t} className={tab === t ? "on" : ""} onClick={() => setTab(t)}>
              {{ report: "Report", http: "HTTP trace", probe: "Stream probe", sources: "Sources" }[t]}
            </button>
          ))}
        </div>
        <div className="flex-1 min-h-0 overflow-auto pt-2">
          {tab === "report" && <ReportTab buildReport={buildReport} />}
          {tab === "http" && <HttpTab />}
          {tab === "probe" && <ProbeTab profile={playback?.profile ?? "stable"} />}
          {tab === "sources" && <SourcesTab />}
        </div>
      </div>
    </div>
  );
}

function Isolation() {
  return (
    <div className="rounded-md p-3 mb-3" style={{ background: "var(--bg-elev-2)", fontSize: 12, lineHeight: 1.6 }}>
      <b>Isolate in this order, changing one variable per test:</b> another channel on the same source works → it's this stream · another
      source works → it's this provider · Ethernet / VPN off changes it → it's the path · stop recordings / multiscreen → it's local load.
      Zap time ≠ live delay: provider HLS windows are often 6–20 s server-side; Low Latency only shrinks <i>our</i> cache.
    </div>
  );
}

function ReportTab({ buildReport }: { buildReport: () => Promise<string> }) {
  const [text, setText] = useState<string>("…");
  const refresh = () => void buildReport().then(setText).catch((e) => setText(String(e)));
  useEffect(refresh, []); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <div>
      <Isolation />
      <div className="flex items-center justify-between mb-1">
        <div className="field" style={{ marginBottom: 0 }}>
          <label>Sanitized report — passwords, tokens, MACs and Xtream path credentials are replaced with ***</label>
        </div>
        <button className="btn ghost" onClick={refresh}>
          <Icon name="refresh" size={14} /> Refresh
        </button>
      </div>
      <Pre text={text} maxHeight={460} />
    </div>
  );
}

function HttpTab() {
  const [rows, setRows] = useState<HttpTrace[]>([]);
  const [sel, setSel] = useState<HttpTrace | null>(null);
  const refresh = () => void ipc.diagHttpTrace().then((r) => setRows(r.slice().reverse())).catch(() => {});
  useEffect(() => {
    refresh();
    const t = window.setInterval(refresh, 2000);
    return () => window.clearInterval(t);
  }, []);
  return (
    <div>
      <div className="flex items-center gap-2 mb-2">
        <div style={{ color: "var(--text-dim)", fontSize: 12 }}>{rows.length} requests (newest first, ring buffer of 250, secrets redacted)</div>
        <div className="flex-1" />
        <button className="btn ghost" onClick={() => void ipc.diagClearTrace().then(refresh)}>
          Clear
        </button>
      </div>
      <div className="rounded-md overflow-auto" style={{ border: "1px solid var(--border)", maxHeight: 300 }}>
        <table className="w-full" style={{ fontSize: 11.5, fontFamily: "var(--mono)", borderCollapse: "collapse" }}>
          <thead style={{ position: "sticky", top: 0, background: "var(--bg-elev)" }}>
            <tr style={{ color: "var(--text-faint)", textAlign: "left" }}>
              <th className="px-2 py-1">time</th>
              <th className="px-2 py-1">kind</th>
              <th className="px-2 py-1">status</th>
              <th className="px-2 py-1">ms</th>
              <th className="px-2 py-1">type</th>
              <th className="px-2 py-1">url</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr
                key={r.id}
                onClick={() => setSel(r)}
                className="cursor-default"
                style={{ background: sel?.id === r.id ? "var(--bg-elev-2)" : undefined, color: r.error ? "var(--danger)" : undefined, borderTop: "1px solid var(--border)" }}
              >
                <td className="px-2 py-0.5 whitespace-nowrap">{new Date(r.at_ms).toLocaleTimeString()}</td>
                <td className="px-2 py-0.5 whitespace-nowrap">{r.kind}</td>
                <td className="px-2 py-0.5">{r.status ?? (r.error ? "ERR" : "…")}</td>
                <td className="px-2 py-0.5">{r.elapsed_ms ?? ""}</td>
                <td className="px-2 py-0.5 whitespace-nowrap" title={r.content_type ?? ""}>
                  {(r.content_type ?? "").split(";")[0].slice(0, 22)}
                </td>
                <td className="px-2 py-0.5 truncate" style={{ maxWidth: 420 }} title={r.url}>
                  {r.method !== "GET" ? `${r.method} ` : ""}
                  {r.url}
                </td>
              </tr>
            ))}
            {rows.length === 0 && (
              <tr>
                <td colSpan={6} className="px-2 py-4 text-center" style={{ color: "var(--text-faint)" }}>
                  No requests yet. Add or refresh a playlist, or run a probe.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {sel && (
        <div className="mt-2 rounded-md p-2" style={{ background: "var(--bg)", border: "1px solid var(--border)", fontSize: 12 }}>
          <div className="mono" style={{ fontFamily: "var(--mono)", wordBreak: "break-all" }}>
            {sel.method} {sel.url}
          </div>
          <div style={{ color: "var(--text-dim)" }}>
            {sel.status ?? "—"} · {sel.elapsed_ms ?? "—"} ms · {sel.content_type ?? "no content-type"}
            {sel.content_length != null && ` · ${sel.content_length.toLocaleString()} B`}
          </div>
          {sel.redirects.map((h, i) => (
            <div key={i} style={{ color: "var(--warn)", fontFamily: "var(--mono)", wordBreak: "break-all" }}>
              ↳ redirect {i + 1}: {h}
            </div>
          ))}
          {sel.error && <div className="flex items-center gap-1" style={{ color: "var(--danger)" }}><Icon name="alert" size={13} /> {sel.error}</div>}
          {sel.preview && (
            <pre className="mt-1 whitespace-pre-wrap" style={{ fontFamily: "var(--mono)", fontSize: 11.5, color: "var(--text-dim)", wordBreak: "break-all" }}>
              {sel.preview}
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

function ProbeTab({ profile }: { profile: string }) {
  const pushToast = useApp((s) => s.pushToast);
  const [url, setUrl] = useState("");
  const [prof, setProf] = useState<"low_latency" | "stable">(profile === "low_latency" ? "low_latency" : "stable");
  const [busy, setBusy] = useState(false);
  const [res, setRes] = useState<ProbeResult | null>(null);
  const valid = /^https?:\/\//i.test(url.trim());

  const probe = async () => {
    setBusy(true);
    setRes(null);
    try {
      setRes(await ipc.diagProbe(url.trim(), prof, 20));
    } catch (e) {
      pushToast({ level: "error", title: "Probe failed", body: String(e) });
    } finally {
      setBusy(false);
    }
  };
  const playHere = async () => {
    try {
      await ipc.loadStream({ stream_url: url.trim(), profile_mode: prof, audio_boost: 100 });
      useApp.setState({ currentChannel: { id: -1, playlist_id: -1, source_id: "probe", name: "URL probe", normalized_name: "", group_title: null, logo: null, stream_url: "", tvg_id: null, catchup_days: 0 } });
      useApp.getState().setUi({ diagnosticsOpen: false });
    } catch (e) {
      pushToast({ level: "error", title: "Could not play", body: String(e) });
    }
  };

  return (
    <div>
      <div className="field">
        <label>Stream URL — probed in an isolated headless libmpv (no window, no audio); the current channel keeps playing</label>
        <div className="flex gap-2">
          <input className="input flex-1" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://…/live/user/pass/123.ts" spellCheck={false} />
          <select className="input" style={{ width: 130 }} value={prof} onChange={(e) => setProf(e.target.value as typeof prof)}>
            <option value="stable">Stable</option>
            <option value="low_latency">Low latency</option>
          </select>
          <button className="btn primary" disabled={!valid || busy} onClick={() => void probe()}>
            {busy ? "Probing…" : "Probe"}
          </button>
          <button className="btn" disabled={!valid || busy} onClick={() => void playHere()} title="Load it in the main player instead">
            Play here
          </button>
        </div>
      </div>
      {busy && (
        <div className="flex items-center gap-2" style={{ color: "var(--text-dim)", fontSize: 12 }}>
          <div className="spinner" /> connecting, demuxing, decoding the first frame (up to 20 s)…
        </div>
      )}
      {res && (
        <div className="rounded-md p-3" style={{ background: "var(--bg-elev-2)", fontSize: 12.5, lineHeight: 1.7 }}>
          <div className="font-semibold flex items-center gap-1.5" style={{ color: res.ok ? "var(--ok)" : "var(--danger)" }}>
            <Icon name={res.ok ? "check" : "alert"} size={15} />
            {res.ok ? `First frame in ${res.ttff_ms} ms` : (res.error ?? "failed")}
          </div>
          <KV k="url" v={res.url} />
          <KV k="container" v={res.container} />
          <KV k="video" v={res.video_codec ? `${res.video_codec}${res.width ? ` ${res.width}×${res.height}` : ""}${res.fps ? ` @ ${res.fps.toFixed(3)} fps` : ""}` : null} />
          <KV k="audio" v={res.audio_codec} />
          <KV k="hwdec" v={res.hwdec} />
          <KV k="cache at first frame" v={res.cache_secs != null ? `${res.cache_secs.toFixed(1)} s` : null} />
          <KV k="profile" v={res.profile} />
          <KV k="engine" v={res.engine} />
          <KV k="total" v={`${res.elapsed_ms} ms`} />
          {res.log.length > 0 && <Pre text={res.log.join("\n")} maxHeight={160} />}
        </div>
      )}
    </div>
  );
}

function SourcesTab() {
  const playlists = useApp((s) => s.playlists);
  const [busy, setBusy] = useState<number | null>(null);
  const [results, setResults] = useState<Record<number, SourceCheck>>({});
  const check = async (id: number) => {
    setBusy(id);
    try {
      const r = await ipc.diagCheckSource(id);
      setResults((m) => ({ ...m, [id]: r }));
    } catch (e) {
      setResults((m) => ({ ...m, [id]: { playlist_id: id, kind: "?", ok: false, elapsed_ms: 0, status: null, content_type: null, content_length: null, final_url: null, payload: null, preview: null, entries: 0, entries_without_url: 0, groups: 0, epg_hint: null, account: null, error: String(e) } }));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div>
      <div style={{ color: "var(--text-dim)", fontSize: 12 }} className="mb-2">
        Read-only checks: fetch + sniff the first 512 bytes + count entries (M3U), or sign in and read account status (Xtream). Nothing is imported.
      </div>
      {playlists.length === 0 && <div style={{ color: "var(--text-faint)" }}>No playlists yet.</div>}
      {playlists.map((p) => {
        const r = results[p.id];
        return (
          <div key={p.id} className="rounded-md p-2 mb-2" style={{ border: "1px solid var(--border)" }}>
            <div className="flex items-center gap-2">
              <span className="font-semibold">{p.name}</span>
              <span className="badge" style={{ fontSize: 11.5 }}>
                {p.type}
              </span>
              <span className="truncate" style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11.5 }}>
                {p.base_url_redacted}
              </span>
              <div className="flex-1" />
              <button className="btn" disabled={busy != null} onClick={() => void check(p.id)}>
                {busy === p.id ? "Checking…" : "Check"}
              </button>
            </div>
            {r && (
              <div className="mt-2" style={{ fontSize: 12, lineHeight: 1.7 }}>
                <div className="font-semibold flex items-center gap-1.5" style={{ color: r.ok ? "var(--ok)" : "var(--danger)" }}>
                  <Icon name={r.ok ? "check" : "alert"} size={15} />
                  {r.ok ? "OK" : (r.error ?? "failed")} <span style={{ color: "var(--text-faint)", fontWeight: 400 }}>· {r.elapsed_ms} ms</span>
                </div>
                {r.kind === "m3u" && (
                  <>
                    <KV k="response" v={`${r.status ?? "—"} · ${r.content_type ?? "no content-type"}${r.content_length != null ? ` · ${r.content_length.toLocaleString()} B` : ""}`} />
                    <KV k="final url" v={r.final_url} />
                    <KV k="payload looks like" v={r.payload} />
                    <KV k="entries" v={`${r.entries.toLocaleString()} with URLs · ${r.entries_without_url} without · ${r.groups} groups`} />
                    <KV k="EPG hint (url-tvg)" v={r.epg_hint} />
                    {r.preview && <Pre text={r.preview} maxHeight={90} />}
                  </>
                )}
                {r.kind === "xtream" && r.account && (
                  <>
                    <KV k="status" v={r.account.status} />
                    <KV k="expires" v={r.account.exp_date ? new Date(r.account.exp_date * 1000).toLocaleString() : "never / unknown"} />
                    <KV k="connections" v={`${r.account.active_connections ?? "?"} active of ${r.account.max_connections ?? "?"} max`} />
                    <KV k="server timezone" v={r.account.server_timezone} />
                  </>
                )}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function KV({ k, v }: { k: string; v: string | null | undefined }) {
  if (v == null || v === "") return null;
  return (
    <div className="flex gap-2">
      <span style={{ color: "var(--text-faint)", minWidth: 150 }}>{k}</span>
      <span style={{ fontFamily: "var(--mono)", wordBreak: "break-all" }}>{v}</span>
    </div>
  );
}

function Pre({ text, maxHeight }: { text: string; maxHeight: number }) {
  return (
    <pre className="rounded-md p-2 overflow-auto whitespace-pre-wrap" style={{ background: "var(--bg)", border: "1px solid var(--border)", maxHeight, fontSize: 11.5, fontFamily: "var(--mono)", color: "var(--text-dim)", wordBreak: "break-all" }}>
      {text}
    </pre>
  );
}
