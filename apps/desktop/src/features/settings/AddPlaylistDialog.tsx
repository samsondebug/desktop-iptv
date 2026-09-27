import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { ipc, type AddPlaylistSource } from "../../lib/ipc";
import { useApp } from "../../lib/store";

type Tab = "xtream" | "url" | "file";

export default function AddPlaylistDialog() {
  const setUi = useApp((s) => s.setUi);
  const pushToast = useApp((s) => s.pushToast);
  const reloadPlaylists = useApp((s) => s.reloadPlaylists);
  const [tab, setTab] = useState<Tab>("xtream");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [ua, setUa] = useState("");
  const [epgUrl, setEpgUrl] = useState("");
  const [path, setPath] = useState("");
  const [base, setBase] = useState("");
  const [user, setUser] = useState("");
  const [pass, setPass] = useState("");
  const [fmt, setFmt] = useState<"ts" | "m3u8">("ts");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const close = () => setUi({ addPlaylistOpen: false });

  const submit = async () => {
    setError(null);
    setBusy(true);
    try {
      let source: AddPlaylistSource;
      if (tab === "xtream") {
        if (!/^https?:\/\//i.test(base.trim()) && !/^[\w.-]+(:\d+)?/.test(base.trim())) throw new Error("Enter the panel URL, e.g. http://host:8080");
        if (!user.trim() || !pass) throw new Error("Username and password are required.");
        source = { kind: "xtream", base_url: base.trim(), username: user.trim(), password: pass, stream_format: fmt, user_agent: ua.trim() || null };
      } else if (tab === "url") {
        if (!/^https?:\/\//i.test(url.trim())) throw new Error("Enter an http(s) URL to an .m3u / .m3u8 playlist.");
        source = { kind: "m3u_url", url: url.trim(), user_agent: ua.trim() || null, epg_url: epgUrl.trim() || null };
      } else {
        if (!path) throw new Error("Choose a playlist file.");
        source = { kind: "m3u_file", path, epg_url: epgUrl.trim() || null };
      }
      await ipc.addPlaylist(name, source);
      await reloadPlaylists();
      pushToast({ level: "info", title: "Import started", body: tab === "xtream" ? "Signing in, then channels → guide → library." : "Channels appear as they are indexed." });
      close();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const pick = async () => {
    const chosen = await open({ multiple: false, directory: false, filters: [{ name: "Playlists", extensions: ["m3u", "m3u8", "txt"] }] });
    if (typeof chosen === "string") setPath(chosen);
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: "min(580px, 92vw)" }}>
        <div className="flex items-center justify-between mb-3">
          <div className="text-[16px] font-semibold">Add playlist</div>
          <div className="seg">
            <button className={tab === "xtream" ? "on" : ""} onClick={() => setTab("xtream")}>
              Xtream
            </button>
            <button className={tab === "url" ? "on" : ""} onClick={() => setTab("url")}>
              M3U URL
            </button>
            <button className={tab === "file" ? "on" : ""} onClick={() => setTab("file")}>
              M3U file
            </button>
            <button disabled title="Days 71–90 (feature-flagged)">
              Stalker
            </button>
          </div>
        </div>

        <div className="flex flex-col gap-3">
          <div className="field">
            <label>Name (optional)</label>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="My provider" />
          </div>
          {tab === "xtream" && (
            <>
              <div className="field">
                <label>Panel URL</label>
                <input className="input" value={base} onChange={(e) => setBase(e.target.value)} placeholder="http://host:8080  (a pasted get.php / player_api.php URL is fine too)" spellCheck={false} autoFocus />
              </div>
              <div className="grid grid-cols-2 gap-3">
                <div className="field">
                  <label>Username</label>
                  <input className="input" value={user} onChange={(e) => setUser(e.target.value)} spellCheck={false} autoComplete="off" />
                </div>
                <div className="field">
                  <label>Password</label>
                  <input className="input" type="password" value={pass} onChange={(e) => setPass(e.target.value)} autoComplete="off" />
                </div>
              </div>
              <div className="field">
                <label>Live stream format</label>
                <div className="seg">
                  <button className={fmt === "ts" ? "on" : ""} onClick={() => setFmt("ts")}>
                    MPEG-TS (.ts) — recommended
                  </button>
                  <button className={fmt === "m3u8" ? "on" : ""} onClick={() => setFmt("m3u8")}>
                    HLS (.m3u8)
                  </button>
                </div>
              </div>
            </>
          )}
          {tab === "url" && (
            <>
              <div className="field">
                <label>Playlist URL</label>
                <input className="input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://host:8080/get.php?username=…&password=…&type=m3u_plus&output=ts" spellCheck={false} autoFocus />
              </div>
              <div className="field">
                <label>EPG / XMLTV URL (optional — auto-detected from url-tvg when present)</label>
                <input className="input" value={epgUrl} onChange={(e) => setEpgUrl(e.target.value)} placeholder="http://host/xmltv.php?… or …/guide.xml.gz" spellCheck={false} />
              </div>
            </>
          )}
          {tab === "file" && (
            <>
              <div className="field">
                <label>File</label>
                <div className="flex gap-2">
                  <input className="input" value={path} readOnly placeholder="Choose an .m3u / .m3u8 file" />
                  <button className="btn" onClick={() => void pick()}>
                    Browse…
                  </button>
                </div>
              </div>
              <div className="field">
                <label>EPG / XMLTV URL (optional)</label>
                <input className="input" value={epgUrl} onChange={(e) => setEpgUrl(e.target.value)} placeholder="http://…/guide.xml" spellCheck={false} />
              </div>
            </>
          )}
          {tab !== "file" && (
            <div className="field">
              <label>User-Agent (optional — some providers require a specific one)</label>
              <input className="input" value={ua} onChange={(e) => setUa(e.target.value)} placeholder="default" spellCheck={false} />
            </div>
          )}
          {error && (
            <div style={{ color: "var(--danger)", fontSize: 12.5 }} className="whitespace-pre-wrap">
              {error}
            </div>
          )}
          <div style={{ color: "var(--text-faint)", fontSize: 11.5, lineHeight: 1.5 }}>
            Credentials stay in the local catalog and are redacted from every log and error. Lists are streamed and indexed; a 40 MB playlist
            never loads into memory at once. If a URL returns a web page instead of a playlist, you will see the first bytes so you can tell what
            the provider sent.
          </div>
        </div>

        <div className="flex justify-end gap-2 mt-4">
          <button className="btn ghost" onClick={close}>
            Cancel
          </button>
          <button className="btn primary" disabled={busy} onClick={() => void submit()}>
            {busy ? "Starting…" : tab === "xtream" ? "Sign in & import" : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}
