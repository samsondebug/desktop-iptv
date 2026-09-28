/**
 * Recordings + downloads — a bottom-pane destination (like the guide or the poster grid), so you
 * keep watching while you manage the library. Rows: status · title/meta · primary action · delete.
 */
import { useEffect, useState } from "react";
import { ipc, type DownloadRecord, type RecordingRecord } from "../../lib/ipc";
import { fmtTime, useApp } from "../../lib/store";
import Icon from "../../components/Icon";

function mb(n: number) {
  return n >= 1e9 ? `${(n / 1e9).toFixed(2)} GB` : `${(n / 1e6).toFixed(0)} MB`;
}

export default function LibraryPane() {
  const pushToast = useApp((s) => s.pushToast);
  const recordingBytes = useApp((s) => s.recordingBytes);
  const downloadProgress = useApp((s) => s.downloadProgress);
  const listVersion = useApp((s) => s.listVersion);
  const [tab, setTab] = useState<"recordings" | "downloads">("recordings");
  const [recs, setRecs] = useState<RecordingRecord[]>([]);
  const [dls, setDls] = useState<DownloadRecord[]>([]);
  const [dir, setDir] = useState("");
  const [tick, setTick] = useState(0);

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
    } catch (e) {
      pushToast({ level: "error", title: "Cannot play", body: String(e) });
    }
  };
  const playDl = async (d: DownloadRecord) => {
    try {
      const pb = await ipc.playDownload(d.id);
      useApp.setState({ playback: pb, currentChannel: null, currentVod: null, currentEpisode: null, currentSeries: null });
    } catch (e) {
      pushToast({ level: "error", title: "Cannot play", body: String(e) });
    }
  };

  const active = recs.filter((r) => r.status === "recording").length;

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-3 px-3 py-1.5 border-b shrink-0" style={{ borderColor: "var(--border)" }}>
        <div className="font-semibold">Library</div>
        <div className="seg">
          <button className={tab === "recordings" ? "on" : ""} onClick={() => setTab("recordings")}>
            Recordings{recs.length ? ` · ${recs.length}` : ""}
            {active > 0 && (
              <span className="ml-1.5" style={{ color: "var(--live)" }}>
                ● {active}
              </span>
            )}
          </button>
          <button className={tab === "downloads" ? "on" : ""} onClick={() => setTab("downloads")}>
            Downloads{dls.length ? ` · ${dls.length}` : ""}
          </button>
        </div>
        <div className="flex-1" />
        <div className="truncate" title={dir} style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11.5, maxWidth: 420 }}>
          {dir}
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-auto p-2 flex flex-col gap-1">
        {tab === "recordings" && recs.length === 0 && (
          <Empty
            icon="record"
            title="No recordings yet"
            body="Press R while watching a channel, click the record button on the player, or right-click a programme in the guide to schedule one. Keep the app running for scheduled jobs."
          />
        )}
        {tab === "recordings" &&
          recs.map((r) => {
            const live = recordingBytes[r.id];
            const bytes = live ?? r.bytes;
            const color = r.status === "recording" ? "var(--live)" : r.status === "failed" ? "var(--danger)" : r.status === "scheduled" ? "var(--warn)" : "var(--ok)";
            return (
              <div key={r.id} className="episode-row" style={{ gridTemplateColumns: "104px 1fr auto auto" }}>
                <div className="flex items-center gap-1.5" style={{ color, fontFamily: "var(--mono)", fontSize: 11.5, textTransform: "uppercase", letterSpacing: "0.04em" }}>
                  {r.status === "recording" && <Icon name="record" size={10} />}
                  {r.status === "recording" ? "REC" : r.status}
                </div>
                <div className="min-w-0">
                  <div className="truncate">{r.title ?? r.channel_name}</div>
                  <div className="truncate" style={{ color: "var(--text-faint)", fontSize: 11.5 }}>
                    {r.channel_name} · {new Date(r.start * 1000).toLocaleDateString()} {fmtTime(r.start)}–{fmtTime(r.stop)}
                    {r.extra_end_s ? ` +${Math.round(r.extra_end_s / 60)}m` : ""} · {mb(bytes)}
                    {r.error && <span style={{ color: "var(--danger)" }}> · {r.error}</span>}
                  </div>
                </div>
                <div className="flex gap-1">
                  {r.status === "recording" && (
                    <button className="btn danger" onClick={() => void ipc.stopRecording(r.id).then(reload)}>
                      <Icon name="stop" size={12} />
                      Stop
                    </button>
                  )}
                  {r.status === "completed" && (
                    <button className="btn" onClick={() => void playRec(r)}>
                      <Icon name="play" size={12} />
                      Play
                    </button>
                  )}
                </div>
                <button className="btn ghost icon" title="Delete recording and its file" aria-label="Delete" onClick={() => void ipc.deleteRecording(r.id, true).then(reload)}>
                  <Icon name="trash" size={15} />
                </button>
              </div>
            );
          })}

        {tab === "downloads" && dls.length === 0 && (
          <Empty icon="download" title="No downloads yet" body="Right-click a movie or use the download button on an episode. Downloads resume after a network drop and keep an .srt sidecar when the provider has one." />
        )}
        {tab === "downloads" &&
          dls.map((d) => {
            const p = downloadProgress[d.id];
            const done = p?.done ?? d.bytes_done;
            const total = p?.total ?? d.bytes_total;
            const pct = total ? Math.min(100, (done / total) * 100) : 0;
            const color = d.status === "downloading" ? "var(--accent)" : d.status === "failed" ? "var(--danger)" : d.status === "completed" ? "var(--ok)" : "var(--warn)";
            return (
              <div key={d.id} className="episode-row" style={{ gridTemplateColumns: "104px 1fr auto auto" }}>
                <div style={{ color, fontFamily: "var(--mono)", fontSize: 11.5, textTransform: "uppercase", letterSpacing: "0.04em" }}>{d.status}</div>
                <div className="min-w-0">
                  <div className="truncate">{d.title}</div>
                  <div className="progress-bar mt-1">
                    <div style={{ width: `${pct}%` }} />
                  </div>
                  <div className="truncate" style={{ color: "var(--text-faint)", fontSize: 11.5 }}>
                    {mb(done)}
                    {total ? ` / ${mb(total)} · ${pct.toFixed(0)}%` : ""}
                    {d.error && <span style={{ color: "var(--danger)" }}> · {d.error}</span>}
                  </div>
                </div>
                <div className="flex gap-1">
                  {d.status === "downloading" && (
                    <button className="btn" onClick={() => void ipc.pauseDownload(d.id).then(reload)}>
                      <Icon name="pause" size={12} />
                      Pause
                    </button>
                  )}
                  {(d.status === "paused" || d.status === "failed" || d.status === "queued") && (
                    <button className="btn" onClick={() => void ipc.resumeDownload(d.id).then(reload)}>
                      <Icon name="download" size={12} />
                      Resume
                    </button>
                  )}
                  {d.status === "completed" && (
                    <button className="btn" onClick={() => void playDl(d)}>
                      <Icon name="play" size={12} />
                      Play
                    </button>
                  )}
                </div>
                <button className="btn ghost icon" title="Delete download and its file" aria-label="Delete" onClick={() => void ipc.deleteDownload(d.id, true).then(reload)}>
                  <Icon name="trash" size={15} />
                </button>
              </div>
            );
          })}
      </div>
    </div>
  );
}

function Empty({ icon, title, body }: { icon: "record" | "download"; title: string; body: string }) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center text-center gap-2 px-6 py-8">
      <div className="flex items-center justify-center rounded-full" style={{ width: 40, height: 40, background: "var(--bg-elev-2)", color: "var(--text-faint)" }}>
        <Icon name={icon} size={18} />
      </div>
      <div className="font-semibold">{title}</div>
      <div style={{ color: "var(--text-dim)", maxWidth: 520, fontSize: 12.5 }}>{body}</div>
    </div>
  );
}
