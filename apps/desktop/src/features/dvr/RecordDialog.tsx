/** Record now / schedule from a programme (CLAUDE.md §6.4: overrun for sports). */
import { useEffect, useState } from "react";
import { ipc } from "../../lib/ipc";
import { fmtTime, useApp } from "../../lib/store";
import Icon from "../../components/Icon";

const PRESETS = [30, 60, 90, 120, 180, 240];

export default function RecordDialog() {
  const dlg = useApp((s) => s.ui.recordDialog)!;
  const setUi = useApp((s) => s.setUi);
  const pushToast = useApp((s) => s.pushToast);
  const [minutes, setMinutes] = useState(60);
  const [extra, setExtra] = useState(10);
  const [title, setTitle] = useState(dlg.programme?.title ?? "");
  const [busy, setBusy] = useState(false);
  const close = () => setUi({ recordDialog: null });
  const prog = dlg.programme;
  const now = Math.floor(Date.now() / 1000);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Enter" && void go();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [minutes, extra, title]);

  const go = async () => {
    setBusy(true);
    try {
      if (prog) {
        await ipc.scheduleRecording(dlg.channel.id, prog.start, prog.stop, title || prog.title, extra * 60);
        pushToast({
          level: "info",
          title: prog.start <= now ? "Recording started" : `Scheduled: ${fmtTime(prog.start)}–${fmtTime(prog.stop)} (+${extra} min)`,
          body: `${dlg.channel.name} · keep the app running`,
        });
      } else {
        const r = await ipc.recordNow(dlg.channel.id, minutes, title || null);
        pushToast({ level: r.warning ? "warn" : "info", title: `Recording ${dlg.channel.name} for ${minutes} min`, body: r.warning ?? r.path });
      }
      useApp.setState((s) => ({ listVersion: s.listVersion + 1 }));
      close();
    } catch (e) {
      pushToast({ level: "error", title: "Could not record", body: String(e), sticky: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: 460 }}>
        <div className="text-[15px] font-semibold flex items-center gap-2"><Icon name="record" size={14} style={{ color: "var(--live)" }} /> Record · {dlg.channel.name}</div>
        {prog ? (
          <div className="mt-1" style={{ color: "var(--text-dim)", fontSize: 12.5 }}>
            {prog.title} · {fmtTime(prog.start)}–{fmtTime(prog.stop)}
            {prog.start <= now && " · already on air — starts now"}
          </div>
        ) : (
          <div className="mt-1" style={{ color: "var(--text-dim)", fontSize: 12.5 }}>
            Starts now.
          </div>
        )}
        <div className="field mt-3">
          <label>Title (optional)</label>
          <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder={prog?.title ?? dlg.channel.name} />
        </div>
        {prog ? (
          <div className="field mt-3">
            <label>End late (overrun) · {extra} min — use 10–30 for sports</label>
            <input type="range" min={0} max={60} step={5} value={extra} onChange={(e) => setExtra(Number(e.target.value))} style={{ accentColor: "var(--accent)" }} />
          </div>
        ) : (
          <div className="field mt-3">
            <label>Duration</label>
            <div className="flex flex-wrap gap-1">
              {PRESETS.map((m) => (
                <button key={m} className={"btn" + (m === minutes ? " primary" : "")} onClick={() => setMinutes(m)}>
                  {m >= 60 ? `${m / 60} h${m % 60 ? ` ${m % 60}` : ""}` : `${m} min`}
                </button>
              ))}
            </div>
          </div>
        )}
        <div className="mt-2" style={{ color: "var(--text-faint)", fontSize: 11.5, lineHeight: 1.5 }}>
          If this channel is playing, the recording shares the playback connection (no extra stream). Otherwise it opens its own connection —
          cheap plans may cap those. Keep the app running for scheduled jobs.
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button className="btn ghost" onClick={close}>
            Cancel
          </button>
          <button className="btn primary" disabled={busy} onClick={() => void go()}>
            {prog && prog.start > now ? "Schedule" : "Record"}
          </button>
        </div>
      </div>
    </div>
  );
}
