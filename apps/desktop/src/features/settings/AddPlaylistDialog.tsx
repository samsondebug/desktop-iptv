import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc } from "../../lib/ipc";
import { useApp } from "../../lib/store";

export default function AddPlaylistDialog() {
  const setUi = useApp((s) => s.setUi);
  const pushToast = useApp((s) => s.pushToast);
  const reloadPlaylists = useApp((s) => s.reloadPlaylists);
  const [tab, setTab] = useState<"url" | "file">("url");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [ua, setUa] = useState("");
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const close = () => setUi({ addPlaylistOpen: false });

  const submit = async () => {
    setError(null);
    setBusy(true);
    try {
      const source =
        tab === "url"
          ? ({ kind: "m3u_url", url: url.trim(), user_agent: ua.trim() || null } as const)
          : ({ kind: "m3u_file", path } as const);
      if (tab === "url" && !/^https?:\/\//i.test(url.trim())) throw new Error("Enter an http(s) URL to an .m3u / .m3u8 playlist.");
      if (tab === "file" && !path) throw new Error("Choose a playlist file.");
      await ipc.addPlaylist(name, source);
      await reloadPlaylists();
      pushToast({ level: "info", title: "Import started", body: "Channels appear as they are indexed." });
      close();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const pick = async () => {
    const chosen = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Playlists", extensions: ["m3u", "m3u8", "txt"] }],
    });
    if (typeof chosen === "string") setPath(chosen);
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: "min(560px, 92vw)" }}>
        <div className="flex items-center justify-between mb-3">
          <div className="text-[16px] font-semibold">Add playlist</div>
          <div className="seg">
            <button className={tab === "url" ? "on" : ""} onClick={() => setTab("url")}>
              M3U URL
            </button>
            <button className={tab === "file" ? "on" : ""} onClick={() => setTab("file")}>
              M3U file
            </button>
            <button disabled title="Days 15–30">
              Xtream
            </button>
            <button disabled title="Days 71–90 (flagged)">
              Stalker
            </button>
          </div>
        </div>

        <div className="flex flex-col gap-3">
          <div className="field">
            <label>Name (optional)</label>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="My provider" />
          </div>
          {tab === "url" ? (
            <>
              <div className="field">
                <label>Playlist URL</label>
                <input
                  className="input"
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  placeholder="http://host:8080/get.php?username=…&password=…&type=m3u_plus&output=ts"
                  spellCheck={false}
                  autoFocus
                />
              </div>
              <div className="field">
                <label>User-Agent (optional — some providers require a specific one)</label>
                <input className="input" value={ua} onChange={(e) => setUa(e.target.value)} placeholder="default" spellCheck={false} />
              </div>
            </>
          ) : (
            <div className="field">
              <label>File</label>
              <div className="flex gap-2">
                <input className="input" value={path} readOnly placeholder="Choose an .m3u / .m3u8 file" />
                <button className="btn" onClick={() => void pick()}>
                  Browse…
                </button>
              </div>
            </div>
          )}
          {error && (
            <div style={{ color: "var(--danger)", fontSize: 12.5 }} className="whitespace-pre-wrap">
              {error}
            </div>
          )}
          <div style={{ color: "var(--text-faint)", fontSize: 11.5, lineHeight: 1.5 }}>
            Credentials in the URL stay in the local catalog and are redacted from every log and error. The list is streamed and indexed;
            a 40 MB playlist never loads into memory at once. If the URL returns a web page instead of a playlist, you will see the first
            bytes so you can tell what the provider sent.
          </div>
        </div>

        <div className="flex justify-end gap-2 mt-4">
          <button className="btn ghost" onClick={close}>
            Cancel
          </button>
          <button className="btn primary" disabled={busy} onClick={() => void submit()}>
            {busy ? "Starting…" : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}
