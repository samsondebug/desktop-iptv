/** Recordings + downloads manager. */
import { useEffect, useState } from "react";
import { ipc, type DownloadRecord, type RecordingRecord } from "../../lib/ipc";
import { fmtTime, useApp } from "../../lib/store";

function mb(n: number) {
  return n >= 1e9 ? `${(n / 1e9).toFixed(2)} GB` : `${(n / 1e6).toFixed(0)} MB`;
}

export default function LibraryModal() {
  const setUi = useApp((s) => s.setUi);
  const pushToast = useApp((s) => s.pushToast);
  const recordingBytes = useApp((s) => s.recordingBytes);
  const downloadProgress = useApp((s) => s.downloadProgress);
  const listVersion = useApp((s) => s.listVersion);
  const [tab, setTab] = useState<"recordings" | "downloads">("recordings");
  const [recs, setRecs] = useState<RecordingRecord[]>([]);
  const [dls, setDls] = useState<DownloadRecord[]>([]);
  const [dir, setDir] = useState("");
  const [tick, setTick] = useState(0);
  const close = () => setUi({ libraryOpen: false });

  const reload = () => {
    ipc.listRecordings().then(setRecs).catch(() => {});
    ipc.listDownloads().then(setDls).catch(() => {});
    ipc.mediaDir().then(setDir).catch(() => {});
  };
  useEffect(reload, [listVersion, tick]);
  useEffect(() => {
    const t = setInterval(() => setTick((x) => x + 1), 5000);
    return () => clearInterval(t);
  }, []);

  const playRec = async (r: RecordingRecord) => {
    try {
      const pb = await ipc.playRecording(r.id);
      useApp.setState({ playback: pb, currentChannel: null, currentVod: null, currentEpisode: null, currentSeries: null });
      close();
    } catch (e) {
      pushToast({ level: "error", title: "Cannot play", body: String(e) });
    }
  };
  const playDl = async (d: DownloadRecord) => {
    try {
      const pb = await ipc.playDownload(d.id);
      useApp.setState({ playback: pb, currentChannel: null, currentVod: null, currentEpisode: null, currentSeries: null });
      close();
    } catch (e) {
      pushToast({ level: "error", title: "Cannot play", body: String(e) });
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: "min(860px, 94vw)", minHeight: 420 }}>
        <div className="flex items-start justify-between mb-2 gap-3">
          <div className="min-w-0">
            <div className="text-[16px] font-semibold">Library</div>
            <div className="truncate" title={dir} style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11 }}>
              {dir}
            </div>
          </div>
          <button className="btn ghost shrink-0" onClick={close}>
            ✕
          </button>
        </div>
        <div className="tabs">
          <button className={tab === "recordings" ? "on" : ""} onClick={() => setTab("recordings")}>
            Recordings ({recs.length})
          </button>
          <button className={tab === "downloads" ? "on" : ""} onClick={() => setTab("downloads")}>
            Downloads ({dls.length})
          </button>
        </div>

        {tab === "recordings" && (
          <div className="flex flex-col gap-1" style={{ maxHeight: 460, overflow: "auto" }}>
            {recs.length === 0 && <div style={{ color: "var(--text-faint)" }}>No recordings. Use ● on the player or right-click a programme in the guide.</div>}
            {recs.map((r) => {
              const live = recordingBytes[r.id];
              const bytes = live ?? r.bytes;
              const color = r.status === "recording" ? "var(--live)" : r.status === "failed" ? "var(--danger)" : r.status === "scheduled" ? "var(--warn)" : "var(--accent-2)";
              return (
                <div key={r.id} className="episode-row" style={{ gridTemplateColumns: "90px 1fr auto auto" }}>
                  <div style={{ color, fontFamily: "var(--mono)", fontSize: 11, textTransform: "uppercase" }}>
                    {r.status === "recording" ? "● REC" : r.status}
                  </div>
                  <div className="min-w-0">
                    <div className="truncate">{r.title ?? r.channel_name}</div>
                    <div className="truncate" style={{ color: "var(--text-faint)", fontSize: 11 }}>
                      {r.channel_name} · {new Date(r.start * 1000).toLocaleDateString()} {fmtTime(r.start)}–{fmtTime(r.stop)}
                      {r.extra_end_s ? ` +${Math.round(r.extra_end_s / 60)}m` : ""} · {mb(bytes)}
                      {r.error && <span style={{ color: "var(--danger)" }}> · {r.error}</span>}
                    </div>
                  </div>
                  <div className="flex gap-1">
                    {r.status === "recording" && (
                      <button className="btn danger" onClick={() => void ipc.stopRecording(r.id).then(reload)}>
                        Stop
                      </button>
                    )}
                    {r.status === "completed" && (
                      <button className="btn" onClick={() => void playRec(r)}>
                        ▶ Play
                      </button>
                    )}
                  </div>
                  <button className="btn ghost" title="Delete (and file)" onClick={() => void ipc.deleteRecording(r.id, true).then(reload)}>
                    🗑
                  </button>
                </div>
              );
            })}
          </div>
        )}

        {tab === "downloads" && (
          <div className="flex flex-col gap-1" style={{ maxHeight: 460, overflow: "auto" }}>
            {dls.length === 0 && <div style={{ color: "var(--text-faint)" }}>No downloads. Right-click a movie or use ⤓ on an episode.</div>}
            {dls.map((d) => {
              const p = downloadProgress[d.id];
              const done = p?.done ?? d.bytes_done;
              const total = p?.total ?? d.bytes_total;
              const pct = total ? Math.min(100, (done / total) * 100) : 0;
              const color = d.status === "downloading" ? "var(--accent)" : d.status === "failed" ? "var(--danger)" : d.status === "completed" ? "var(--accent-2)" : "var(--warn)";
              return (
                <div key={d.id} className="episode-row" style={{ gridTemplateColumns: "90px 1fr auto auto" }}>
                  <div style={{ color, fontFamily: "var(--mono)", fontSize: 11, textTransform: "uppercase" }}>{d.status}</div>
                  <div className="min-w-0">
                    <div className="truncate">{d.title}</div>
                    <div className="progress-bar mt-1">
                      <div style={{ width: `${pct}%` }} />
                    </div>
                    <div className="truncate" style={{ color: "var(--text-faint)", fontSize: 11 }}>
                      {mb(done)}
                      {total ? ` / ${mb(total)} · ${pct.toFixed(0)}%` : ""}
                      {d.error && <span style={{ color: "var(--danger)" }}> · {d.error}</span>}
                    </div>
                  </div>
                  <div className="flex gap-1">
                    {d.status === "downloading" && (
                      <button className="btn" onClick={() => void ipc.pauseDownload(d.id).then(reload)}>
                        Pause
                      </button>
                    )}
                    {(d.status === "paused" || d.status === "failed" || d.status === "queued") && (
                      <button className="btn" onClick={() => void ipc.resumeDownload(d.id).then(reload)}>
                        Resume
                      </button>
                    )}
                    {d.status === "completed" && (
                      <button className="btn" onClick={() => void playDl(d)}>
                        ▶ Play
                      </button>
                    )}
                  </div>
                  <button className="btn ghost" title="Delete (and file)" onClick={() => void ipc.deleteDownload(d.id, true).then(reload)}>
                    🗑
                  </button>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
