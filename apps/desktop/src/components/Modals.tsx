/** Small modals: resume prompt, parental unlock, EPG override editor. */
import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmtDuration, useApp } from "../lib/store";

export function ResumePromptModal() {
  const prompt = useApp((s) => s.ui.resumePrompt)!;
  const setUi = useApp((s) => s.setUi);
  const playVod = useApp((s) => s.playVod);
  const playEpisode = useApp((s) => s.playEpisode);
  const currentSeries = useApp((s) => s.currentSeries);

  const go = async (fromStart: boolean) => {
    setUi({ resumePrompt: null });
    if (prompt.kind === "vod") {
      const v = prompt.vod ?? (await ipc.getVod(prompt.id));
      await playVod(v, fromStart);
    } else if (prompt.episode) {
      await playEpisode(prompt.episode, prompt.series ?? currentSeries, fromStart);
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Enter") void go(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setUi({ resumePrompt: null })}>
      <div className="modal" style={{ width: 420 }}>
        <div className="text-[15px] font-semibold">{prompt.title}</div>
        <div className="mt-1" style={{ color: "var(--text-dim)" }}>
          You stopped at {fmtDuration(prompt.position_s)}
          {prompt.duration_s ? ` of ${fmtDuration(prompt.duration_s)}` : ""}.
        </div>
        <div className="mt-4 flex justify-end gap-2">
          <button className="btn" onClick={() => void go(true)}>
            Start over
          </button>
          <button className="btn primary" onClick={() => void go(false)}>
            Resume
          </button>
        </div>
      </div>
    </div>
  );
}

export function UnlockModal() {
  const setUi = useApp((s) => s.setUi);
  const refreshParental = useApp((s) => s.refreshParental);
  const pushToast = useApp((s) => s.pushToast);
  const [pin, setPin] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const submit = async () => {
    try {
      await ipc.unlockParental(pin);
      await refreshParental();
      setUi({ unlockOpen: false });
      pushToast({ level: "info", title: "Parental filter unlocked for this session" });
    } catch (e) {
      setErr(String(e));
      setPin("");
    }
  };
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setUi({ unlockOpen: false })}>
      <div className="modal" style={{ width: 360 }}>
        <div className="text-[15px] font-semibold">Enter PIN</div>
        <input
          className="input mt-3"
          type="password"
          inputMode="numeric"
          autoFocus
          value={pin}
          onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))}
          onKeyDown={(e) => e.key === "Enter" && void submit()}
          placeholder="••••"
          style={{ fontSize: 20, letterSpacing: "0.3em", textAlign: "center" }}
        />
        {err && (
          <div className="mt-2" style={{ color: "var(--danger)", fontSize: 12 }}>
            {err}
          </div>
        )}
        <div className="mt-4 flex justify-end gap-2">
          <button className="btn ghost" onClick={() => setUi({ unlockOpen: false })}>
            Cancel
          </button>
          <button className="btn primary" onClick={() => void submit()}>
            Unlock
          </button>
        </div>
      </div>
    </div>
  );
}

export function EpgEditModal() {
  const ch = useApp((s) => s.ui.epgEditChannel)!;
  const setUi = useApp((s) => s.setUi);
  const playlistId = useApp((s) => s.activePlaylistId);
  const pushToast = useApp((s) => s.pushToast);
  const [current, setCurrent] = useState<string | null>(null);
  const [q, setQ] = useState(ch.tvg_id ?? ch.name.split(/[\s|:]+/)[0] ?? "");
  const [ids, setIds] = useState<string[]>([]);
  const [value, setValue] = useState("");

  useEffect(() => {
    ipc.getEpgOverride(ch.id).then((o) => {
      setCurrent(o);
      setValue(o ?? ch.tvg_id ?? "");
    });
  }, [ch]);

  useEffect(() => {
    if (playlistId == null) return;
    const t = window.setTimeout(() => ipc.epgSearchIds(playlistId, q).then(setIds).catch(() => setIds([])), 120);
    return () => window.clearTimeout(t);
  }, [q, playlistId]);

  const save = async (v: string | null) => {
    await ipc.setEpgOverride(ch.id, v);
    useApp.setState((s) => ({ epgVersion: s.epgVersion + 1 }));
    pushToast({ level: "info", title: v ? `Guide for “${ch.name}” now uses ${v}` : `Guide override removed for “${ch.name}”` });
    setUi({ epgEditChannel: null });
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && setUi({ epgEditChannel: null })}>
      <div className="modal" style={{ width: 520 }}>
        <div className="text-[15px] font-semibold">Edit EPG · {ch.name}</div>
        <div className="mt-1" style={{ color: "var(--text-dim)", fontSize: 12 }}>
          Playlist tvg-id: <code>{ch.tvg_id ?? "—"}</code>
          {current && (
            <>
              {" "}
              · override: <code>{current}</code>
            </>
          )}
        </div>
        <div className="field mt-3">
          <label>Use this guide id</label>
          <input className="input" value={value} onChange={(e) => setValue(e.target.value)} placeholder="e.g. ESPN.us" spellCheck={false} />
        </div>
        <div className="field mt-3">
          <label>Find ids in the imported guide</label>
          <input className="input" value={q} onChange={(e) => setQ(e.target.value)} placeholder="type part of an id…" spellCheck={false} autoFocus />
          <div className="mt-1 flex flex-wrap gap-1" style={{ maxHeight: 160, overflow: "auto" }}>
            {ids.map((id) => (
              <button key={id} className="btn" style={{ padding: "2px 8px", fontSize: 11.5 }} onClick={() => setValue(id)}>
                {id}
              </button>
            ))}
            {ids.length === 0 && <span style={{ color: "var(--text-faint)", fontSize: 12 }}>No matching ids (import a guide first).</span>}
          </div>
        </div>
        <div className="mt-4 flex justify-between">
          <button className="btn ghost" onClick={() => void save(null)} disabled={!current}>
            Remove override
          </button>
          <div className="flex gap-2">
            <button className="btn ghost" onClick={() => setUi({ epgEditChannel: null })}>
              Cancel
            </button>
            <button className="btn primary" onClick={() => void save(value.trim() || null)}>
              Save
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
