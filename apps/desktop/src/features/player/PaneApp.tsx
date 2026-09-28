/**
 * Multiscreen pane window (`?pane=<label>`): a transparent surface for its own libmpv instance,
 * a channel picker, and an audio-lock toggle (CLAUDE.md §6.5). Muted unless it holds audio.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { events, ipc, telemetryOf, type ChannelRecord, type EngineTelemetryEvent, type PaneInfo, type PlaylistSummary } from "../../lib/ipc";
import { shortCodec } from "./Hud";
import Icon from "../../components/Icon";

export default function PaneApp({ label }: { label: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [playlists, setPlaylists] = useState<PlaylistSummary[]>([]);
  const [playlistId, setPlaylistId] = useState<number | null>(null);
  const [q, setQ] = useState("");
  const [rows, setRows] = useState<ChannelRecord[]>([]);
  const [current, setCurrent] = useState<ChannelRecord | null>(null);
  const [info, setInfo] = useState<PaneInfo | null>(null);
  const [tele, setTele] = useState<EngineTelemetryEvent | null>(null);
  const [pickerOpen, setPickerOpen] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const report = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    void ipc.paneSetVideoRect(label, r.left, r.top, r.width, r.height, window.innerWidth, window.innerHeight).catch(() => {});
  }, [label]);

  useLayoutEffect(() => {
    report();
    const ro = new ResizeObserver(() => report());
    if (ref.current) ro.observe(ref.current);
    window.addEventListener("resize", report);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", report);
    };
  }, [report, pickerOpen]);

  useEffect(() => {
    ipc.listPlaylists().then((p) => {
      setPlaylists(p);
      setPlaylistId(p[0]?.id ?? null);
    });
    const refresh = () => ipc.listPanes().then((ps) => setInfo(ps.find((p) => p.label === label) ?? null)).catch(() => {});
    void refresh();
    const un1 = events.onPanesChanged((ps) => setInfo(ps.find((p) => p.label === label) ?? null));
    const un2 = events.onPaneEngine((l, ev) => {
      if (l !== label) return;
      const t = telemetryOf(ev);
      if (t) setTele(t);
      else if (ev.type === "end_file" && ev.error) setError(ev.error);
      else if (ev.type === "playback_started") setError(null);
    });
    return () => {
      void un1.then((f) => f());
      void un2.then((f) => f());
    };
  }, [label]);

  // The pane may have been opened already tuned (main window "open pane with this channel"):
  // follow the backend's channel id so the header and the picker highlight match.
  const infoChannelId = info?.channel_id ?? null;
  useEffect(() => {
    if (infoChannelId == null) {
      if (info && !info.playing) setCurrent(null);
      return;
    }
    if (current?.id === infoChannelId) return;
    ipc.getChannel(infoChannelId).then(setCurrent).catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [infoChannelId]);

  useEffect(() => {
    if (playlistId == null) return;
    const t = window.setTimeout(() => {
      const p = q.trim()
        ? ipc.searchChannels({ query_string: q.trim(), playlist_id: playlistId, limit: 200, offset: 0 })
        : ipc.listChannels({ playlist_id: playlistId, group_title: null, limit: 200, offset: 0 });
      p.then(setRows).catch(() => setRows([]));
    }, 120);
    return () => window.clearTimeout(t);
  }, [q, playlistId]);

  const play = async (ch: ChannelRecord) => {
    setCurrent(ch);
    setError(null);
    await ipc.panePlay(label, ch.id).catch((e) => setError(String(e)));
    setPickerOpen(false);
  };

  return (
    <div className="h-full w-full flex flex-col" style={{ background: "transparent" }}>
      <div className="opaque flex items-center gap-2 px-2 border-b shrink-0" style={{ height: 36, borderColor: "var(--border)" }}>
        <span className="font-semibold" style={{ fontSize: 12.5 }}>
          Pane {label.replace(/^pane-/, "")}
        </span>
        <span className="truncate" style={{ color: current ? "var(--text)" : "var(--text-faint)", fontSize: 12.5 }}>
          {current?.name ?? "No channel"}
        </span>
        <div className="flex-1" />
        {current && tele && tele.width > 0 && (
          <span className="hud" style={{ fontSize: 11.5 }}>
            {tele.width}x{tele.height} · {shortCodec(tele.codec_name)} · {tele.fps ? Math.round(tele.fps) + "fps" : ""}
          </span>
        )}
        <button className="btn" onClick={() => setPickerOpen((v) => !v)} title="Choose channel">
          <Icon name="tv" size={14} />
          Channel
          <Icon name={pickerOpen ? "chevronUp" : "chevronDown"} size={14} />
        </button>
        <button
          className={"btn icon" + (info?.has_audio ? " primary" : " ghost")}
          title={info?.has_audio ? "Audio is on this pane — click to give it back to the main player" : "Give audio to this pane"}
          aria-label="Audio lock"
          onClick={() => void ipc.paneAudio(info?.has_audio ? "main" : label)}
        >
          <Icon name={info?.has_audio ? "volume" : "muted"} size={16} />
        </button>
        <button className="btn ghost icon" onClick={() => void ipc.paneStop(label)} title="Stop" aria-label="Stop">
          <Icon name="stop" size={14} />
        </button>
        <button className="btn ghost icon" onClick={() => void ipc.paneClose(label)} title="Close pane" aria-label="Close pane">
          <Icon name="close" size={16} />
        </button>
      </div>
      <div className="flex-1 min-h-0 flex">
        <div ref={ref} className="flex-1 relative" style={{ background: "transparent" }}>
          {!current && (
            <div className="absolute inset-0 flex items-center justify-center pointer-events-none" style={{ color: "var(--text-faint)", fontSize: 12 }}>
              Pick a channel
            </div>
          )}
          {error && (
            <div className="absolute left-2 right-2 bottom-2 hud" style={{ borderLeft: "3px solid var(--danger)" }}>
              {error}
            </div>
          )}
        </div>
        {pickerOpen && (
          <div className="opaque border-l flex flex-col" style={{ width: 240, borderColor: "var(--border)" }}>
            <div className="p-1.5 flex flex-col gap-1">
              <select className="input" value={playlistId ?? ""} onChange={(e) => setPlaylistId(Number(e.target.value))}>
                {playlists.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
              <input className="input" placeholder="Search…" value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} />
            </div>
            <div className="flex-1 min-h-0 overflow-auto">
              {rows.map((ch) => (
                <div key={ch.id} className={"rail-item" + (current?.id === ch.id ? " active" : "")} style={{ margin: "0 6px" }} onClick={() => void play(ch)} title={ch.group_title ?? ""}>
                  <span className="truncate">{ch.name}</span>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
