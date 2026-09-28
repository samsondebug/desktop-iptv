import { useEffect, useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { ipc, type ConfigPayload, type EpgSource, type HwDecoding, type PlaylistMeta, type XtreamAccount } from "../../lib/ipc";
import { useApp } from "../../lib/store";
import Icon from "../../components/Icon";

const HWDEC: { v: HwDecoding; label: string; os?: string[] }[] = [
  { v: "auto-safe", label: "Auto (safe) — recommended" },
  { v: "d3d11va", label: "D3D11VA zero-copy", os: ["windows"] },
  { v: "d3d11va-copy", label: "D3D11VA copy-back (multiscreen / filters)", os: ["windows"] },
  { v: "videotoolbox", label: "VideoToolbox", os: ["macos"] },
  { v: "videotoolbox-copy", label: "VideoToolbox copy-back", os: ["macos"] },
  { v: "vaapi", label: "VA-API", os: ["linux"] },
  { v: "vaapi-copy", label: "VA-API copy-back", os: ["linux"] },
  { v: "no", label: "Software decoding" },
];

const TABS: { id: ReturnType<typeof useApp.getState>["ui"]["settingsTab"]; label: string }[] = [
  { id: "playback", label: "Playback" },
  { id: "interface", label: "Interface" },
  { id: "playlists", label: "Playlists" },
  { id: "guide", label: "Guide" },
  { id: "parental", label: "Parental" },
  { id: "license", label: "License" },
  { id: "about", label: "About & backup" },
];

export default function SettingsDrawer() {
  const boot = useApp((s) => s.boot)!;
  const config = useApp((s) => s.config)!;
  const saveConfig = useApp((s) => s.saveConfig);
  const setUi = useApp((s) => s.setUi);
  const tab = useApp((s) => s.ui.settingsTab);
  const [draft, setDraft] = useState<ConfigPayload>(config);

  const set = <K extends keyof ConfigPayload>(k: K, v: ConfigPayload[K]) => {
    setDraft((d) => ({ ...d, [k]: v }));
    void saveConfig({ [k]: v } as Partial<ConfigPayload>);
  };
  const close = () => setUi({ settingsOpen: false });
  const hwOptions = HWDEC.filter((h) => !h.os || h.os.includes(boot.platform));

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: "min(820px, 94vw)", minHeight: 520 }}>
        <div className="flex items-center justify-between mb-2">
          <div className="text-[16px] font-semibold">Settings</div>
          <button className="btn ghost" onClick={close}>
            <Icon name="close" size={16} />
          </button>
        </div>
        <div className="tabs">
          {TABS.map((t) => (
            <button key={t.id} className={tab === t.id ? "on" : ""} onClick={() => setUi({ settingsTab: t.id })}>
              {t.label}
            </button>
          ))}
        </div>

        {tab === "playback" && (
          <section className="grid grid-cols-2 gap-x-6 gap-y-4">
            <div className="field">
              <label>Default profile</label>
              <div className="seg">
                <button className={draft.default_profile === "stable" ? "on" : ""} onClick={() => set("default_profile", "stable")}>
                  Stable (20–60 s cache)
                </button>
                <button className={draft.default_profile === "low_latency" ? "on" : ""} onClick={() => set("default_profile", "low_latency")}>
                  Low latency (3 s)
                </button>
              </div>
            </div>
            <div className="field">
              <label>Stable cache · {draft.stable_cache_secs} s</label>
              <input type="range" min={20} max={60} value={draft.stable_cache_secs} onChange={(e) => set("stable_cache_secs", Number(e.target.value))} style={{ accentColor: "var(--accent)" }} />
            </div>
            <div className="field">
              <label>Hardware decoding (applies on restart)</label>
              <select className="input" value={draft.hw_decoding} onChange={(e) => set("hw_decoding", e.target.value)}>
                {hwOptions.map((h) => (
                  <option key={h.v} value={h.v}>
                    {h.label}
                  </option>
                ))}
              </select>
            </div>
            <div className="field">
              <label>Audio boost · {draft.audio_boost}%</label>
              <input type="range" min={100} max={130} value={draft.audio_boost} onChange={(e) => set("audio_boost", Number(e.target.value))} style={{ accentColor: "var(--accent)" }} />
            </div>
            <div className="field">
              <label>Audio delay · {draft.audio_delay_ms} ms</label>
              <input type="range" min={-1000} max={1000} step={10} value={draft.audio_delay_ms} onChange={(e) => set("audio_delay_ms", Number(e.target.value))} style={{ accentColor: "var(--accent)" }} />
            </div>
            <div className="flex flex-col gap-2 pt-4">
              <label className="flex items-center gap-2">
                <input type="checkbox" checked={draft.auto_play_last} onChange={(e) => set("auto_play_last", e.target.checked)} />
                Auto-play last channel on launch
              </label>
              <label className="flex items-center gap-2">
                <input type="checkbox" checked={draft.hud_enabled} onChange={(e) => set("hud_enabled", e.target.checked)} />
                Always show the stats HUD on the player
              </label>
            </div>
            <AdvancedMpv />
          </section>
        )}

        {tab === "interface" && <InterfaceTab draft={draft} set={set} />}
        {tab === "playlists" && <PlaylistsTab draft={draft} set={set} />}
        {tab === "guide" && <GuideTab />}
        {tab === "parental" && <ParentalTab />}
        {tab === "license" && <LicenseTab />}
        {tab === "about" && <AboutTab />}
      </div>
    </div>
  );
}

// ---------- Playback: advanced mpv option map ----------

const ADVANCED_KEYS = ["cache-secs", "demuxer-readahead-secs", "demuxer-max-bytes", "hwdec", "deband", "interpolation", "scale", "network-timeout", "video-sync", "audio-delay", "sub-font-size"];

function AdvancedMpv() {
  const [key, setKey] = useState("cache-secs");
  const [val, setVal] = useState("");
  const [cur, setCur] = useState<string | null>(null);
  const pushToast = useApp((s) => s.pushToast);
  useEffect(() => {
    ipc.engineGetProperty(key).then(setCur).catch(() => setCur(null));
  }, [key]);
  return (
    <div className="col-span-2 rounded-md p-3" style={{ background: "var(--bg-elev-2)" }}>
      <div className="font-semibold mb-1" style={{ fontSize: 12.5 }}>
        Advanced mpv option (session only)
      </div>
      <div className="flex gap-2 items-center">
        <select className="input" style={{ width: 220 }} value={key} onChange={(e) => setKey(e.target.value)}>
          {ADVANCED_KEYS.map((k) => (
            <option key={k}>{k}</option>
          ))}
        </select>
        <span style={{ fontFamily: "var(--mono)", fontSize: 11.5, color: "var(--text-faint)", minWidth: 120 }}>now: {cur ?? "—"}</span>
        <input className="input" style={{ width: 160 }} value={val} onChange={(e) => setVal(e.target.value)} placeholder="value" spellCheck={false} />
        <button
          className="btn"
          onClick={() =>
            void ipc
              .engineSetProperty(key, val)
              .then(() => ipc.engineGetProperty(key).then(setCur))
              .catch((e) => pushToast({ level: "error", title: "mpv rejected that", body: String(e) }))
          }
        >
          Apply
        </button>
      </div>
      <div className="mt-1" style={{ color: "var(--text-faint)", fontSize: 11.5 }}>
        The two profiles cover 99% of cases; this is the documented escape hatch (CLAUDE.md §6.2). Changes reset when the app restarts.
      </div>
    </div>
  );
}

// ---------- Interface ----------

function InterfaceTab({ draft, set }: { draft: ConfigPayload; set: <K extends keyof ConfigPayload>(k: K, v: ConfigPayload[K]) => void }) {
  const [tokens, setTokens] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  const saveConfig = useApp((s) => s.saveConfig);
  useEffect(() => {
    ipc.getThemeTokens().then((t) => setTokens(t ?? "")).catch(() => {});
  }, []);
  const example = JSON.stringify({ "--accent": "#ff8c42", "--bg": "#000000", "--bg-elev": "#0d0d0d", "--live": "#ff3b5c" }, null, 2);
  const save = async (json: string | null) => {
    setMsg(null);
    try {
      await ipc.setThemeTokens(json);
      setTokens(json ?? "");
      await saveConfig({}); // re-trigger the theme effect
      setMsg(json ? "Applied." : "Reset to the built-in theme.");
    } catch (e) {
      setMsg(String(e));
    }
  };
  return (
    <section className="grid grid-cols-2 gap-x-6 gap-y-4">
      <div className="flex flex-col gap-3">
        <div className="field">
          <label>Theme</label>
          <div className="seg">
            <button className={draft.app_theme === "dark" ? "on" : ""} onClick={() => set("app_theme", "dark")}>
              Dark
            </button>
            <button className={draft.app_theme === "light" ? "on" : ""} onClick={() => set("app_theme", "light")}>
              Light
            </button>
          </div>
        </div>
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={draft.hide_vod_tabs} onChange={(e) => set("hide_vod_tabs", e.target.checked)} />
          Hide Movies / Series tabs
        </label>
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={draft.hide_brand_chrome} onChange={(e) => set("hide_brand_chrome", e.target.checked)} />
          Hide brand chrome
        </label>
      </div>
      <div className="field">
        <label>Theme tokens (JSON of CSS variables — exportable, shareable)</label>
        <textarea className="input" rows={9} value={tokens} onChange={(e) => setTokens(e.target.value)} placeholder={example} spellCheck={false} style={{ fontFamily: "var(--mono)", fontSize: 11.5 }} />
        <div className="flex gap-2">
          <button className="btn primary" onClick={() => void save(tokens.trim() || null)}>
            Apply
          </button>
          <button className="btn" onClick={() => setTokens(example)}>
            Example
          </button>
          <button className="btn ghost" onClick={() => void save(null)}>
            Reset
          </button>
          <button className="btn ghost" onClick={() => void navigator.clipboard.writeText(tokens)}>
            Copy
          </button>
        </div>
        {msg && <div style={{ fontSize: 12, color: msg.startsWith("Applied") || msg.startsWith("Reset") ? "var(--accent-2)" : "var(--danger)" }}>{msg}</div>}
      </div>
    </section>
  );
}

// ---------- Playlists ----------

function PlaylistsTab({ draft, set }: { draft: ConfigPayload; set: <K extends keyof ConfigPayload>(k: K, v: ConfigPayload[K]) => void }) {
  const playlists = useApp((s) => s.playlists);
  const reloadPlaylists = useApp((s) => s.reloadPlaylists);
  const pushToast = useApp((s) => s.pushToast);
  const imports = useApp((s) => s.imports);
  const [metas, setMetas] = useState<Record<number, PlaylistMeta>>({});
  const [renaming, setRenaming] = useState<{ id: number; name: string } | null>(null);
  const listVersion = useApp((s) => s.listVersion);

  useEffect(() => {
    Promise.all(playlists.map((p) => ipc.playlistMeta(p.id).then((m) => [p.id, m] as const).catch(() => null))).then((rows) => {
      const m: Record<number, PlaylistMeta> = {};
      for (const r of rows) if (r) m[r[0]] = r[1];
      setMetas(m);
    });
  }, [playlists, listVersion]);

  return (
    <section className="flex flex-col gap-2">
      {playlists.length === 0 && <div style={{ color: "var(--text-faint)" }}>None yet.</div>}
      <label className="flex items-center gap-2 pb-2" style={{ fontSize: 12.5 }}>
        <input type="checkbox" checked={draft.stalker_enabled} onChange={(e) => set("stalker_enabled", e.target.checked)} />
        <span>
          Experimental: Stalker / MAC portal sources <span style={{ color: "var(--text-faint)" }}>— live TV only; links are resolved at play time; no VOD/EPG yet</span>
        </span>
      </label>
      {playlists.map((p) => {
        const meta = metas[p.id];
        const acct: XtreamAccount | null = meta?.account_json ? (JSON.parse(meta.account_json) as XtreamAccount) : null;
        const busy = !!imports[p.id];
        return (
          <div key={p.id} className="rounded-md p-3" style={{ background: "var(--bg-elev-2)" }}>
            <div className="flex items-center gap-2">
              <div className="min-w-0 flex-1">
                {renaming?.id === p.id ? (
                  <input
                    className="input"
                    value={renaming.name}
                    autoFocus
                    onChange={(e) => setRenaming({ id: p.id, name: e.target.value })}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") void ipc.renamePlaylist(p.id, renaming.name).then(reloadPlaylists).then(() => setRenaming(null));
                      if (e.key === "Escape") setRenaming(null);
                    }}
                    onBlur={() => setRenaming(null)}
                  />
                ) : (
                  <div className="truncate font-medium" onDoubleClick={() => setRenaming({ id: p.id, name: p.name })} title="Double-click to rename">
                    {p.name} <span className="kbd">{p.type}</span>
                  </div>
                )}
                <div className="truncate" style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 11.5 }}>
                  {p.base_url_redacted}
                </div>
              </div>
              <span style={{ color: "var(--text-faint)", fontSize: 11.5, fontFamily: "var(--mono)" }}>{p.channel_count.toLocaleString()} ch</span>
              <button className="btn ghost" title="Refresh channels, guide and library" disabled={busy} onClick={() => void ipc.refreshPlaylist(p.id)}>
                {busy ? <span className="spinner" style={{ width: 12, height: 12, borderWidth: 2 }} /> : <Icon name="refresh" size={15} />}
              </button>
              <button className="btn ghost" title="Remove" onClick={() => void ipc.deletePlaylist(p.id).then(reloadPlaylists).then(() => pushToast({ level: "info", title: `Removed ${p.name}` }))}>
                <Icon name="trash" size={15} />
              </button>
            </div>
            {meta && (
              <div className="mt-2 grid grid-cols-2 gap-x-4 gap-y-1" style={{ fontSize: 12 }}>
                <div style={{ color: "var(--text-faint)" }}>
                  last sync: <span style={{ color: "var(--text)" }}>{meta.last_synced ? meta.last_synced.replace("T", " ") : "—"}</span>
                  {meta.last_error && <span style={{ color: "var(--danger)" }}> · {meta.last_error}</span>}
                </div>
                {p.type === "xtream" && (
                  <div className="flex items-center gap-2" style={{ color: "var(--text-faint)" }}>
                    live format
                    <div className="seg">
                      <button className={meta.stream_format === "ts" ? "on" : ""} onClick={() => void ipc.setStreamFormat(p.id, "ts")}>
                        .ts
                      </button>
                      <button className={meta.stream_format === "m3u8" ? "on" : ""} onClick={() => void ipc.setStreamFormat(p.id, "m3u8")}>
                        .m3u8
                      </button>
                    </div>
                  </div>
                )}
                {acct && (
                  <div className="col-span-2 flex flex-wrap gap-x-4 gap-y-1" style={{ fontFamily: "var(--mono)", fontSize: 11.5 }}>
                    <span>
                      status <b style={{ color: acct.status === "Active" ? "var(--accent-2)" : "var(--warn)" }}>{acct.status ?? "?"}</b>
                    </span>
                    <span>expires {acct.exp_date ? new Date(acct.exp_date * 1000).toLocaleDateString() : "never / unknown"}</span>
                    <span>
                      connections {acct.active_cons ?? "?"} / {acct.max_connections ?? "?"}
                    </span>
                    {acct.is_trial && <span style={{ color: "var(--warn)" }}>provider trial</span>}
                    {acct.timezone && <span>tz {acct.timezone}</span>}
                  </div>
                )}
              </div>
            )}
          </div>
        );
      })}
    </section>
  );
}

// ---------- Guide ----------

function GuideTab() {
  const playlists = useApp((s) => s.playlists);
  const activePlaylistId = useApp((s) => s.activePlaylistId);
  const epgStats = useApp((s) => s.epgStats);
  const reloadEpgStats = useApp((s) => s.reloadEpgStats);
  const pushToast = useApp((s) => s.pushToast);
  const epgVersion = useApp((s) => s.epgVersion);
  const [pid, setPid] = useState<number | null>(activePlaylistId);
  const [sources, setSources] = useState<EpgSource[]>([]);
  const [url, setUrl] = useState("");
  const [offset, setOffset] = useState(0);
  const p = playlists.find((x) => x.id === pid);

  useEffect(() => {
    if (pid == null) return;
    ipc.listEpgSources(pid).then(setSources).catch(() => setSources([]));
    ipc.epgStats(pid).then((s) => setOffset(s.offset_min)).catch(() => {});
  }, [pid, epgVersion]);

  const applyOffset = async (m: number) => {
    if (pid == null) return;
    setOffset(m);
    await ipc.setEpgOffset(pid, m);
    useApp.setState((s) => ({ epgVersion: s.epgVersion + 1 }));
    await reloadEpgStats();
  };

  return (
    <section className="flex flex-col gap-4">
      <div className="field">
        <label>Playlist</label>
        <select className="input" style={{ width: 300 }} value={pid ?? ""} onChange={(e) => setPid(Number(e.target.value))}>
          {playlists.map((x) => (
            <option key={x.id} value={x.id}>
              {x.name}
            </option>
          ))}
        </select>
      </div>
      {p && (
        <>
          <div className="rounded-md p-3" style={{ background: "var(--bg-elev-2)", fontSize: 12 }}>
            {pid === activePlaylistId && epgStats ? (
              <>
                <b>{epgStats.programmes.toLocaleString()}</b> programmes · <b>{epgStats.channels_with_epg.toLocaleString()}</b> channels with guide
                {epgStats.max_stop && <> · through {new Date(epgStats.max_stop * 1000).toLocaleString()}</>}
              </>
            ) : (
              "Select the playlist in the top bar to see its guide stats."
            )}
            <div className="mt-2 flex gap-2">
              <button className="btn" onClick={() => void ipc.refreshEpg(p.id).then((ok) => pushToast({ level: "info", title: ok ? "Guide refresh started" : "A sync is already running" }))}>
                <Icon name="refresh" size={14} /> Refresh guide now
              </button>
            </div>
          </div>

          <div className="field">
            <label>Time offset · {offset >= 0 ? "+" : ""}{offset} min (use when the guide is shifted vs. what you see)</label>
            <div className="flex items-center gap-2">
              <input type="range" min={-720} max={720} step={30} value={offset} onChange={(e) => void applyOffset(Number(e.target.value))} style={{ accentColor: "var(--accent)", flex: 1 }} />
              <button className="btn ghost" onClick={() => void applyOffset(0)}>
                0
              </button>
            </div>
          </div>

          {p.type !== "xtream" && (
            <div className="field">
              <label>XMLTV sources (multiple allowed — they merge)</label>
              {sources.map((s) => (
                <div key={s.id} className="flex items-center gap-2 rounded-md px-2 py-1" style={{ background: "var(--bg-elev-2)", fontSize: 12 }}>
                  <div className="min-w-0 flex-1">
                    <div className="truncate" style={{ fontFamily: "var(--mono)", fontSize: 11.5 }}>
                      {s.url_redacted}
                    </div>
                    <div style={{ color: s.last_error ? "var(--danger)" : "var(--text-faint)", fontSize: 11.5 }}>
                      {s.last_error ?? (s.last_synced ? `${s.programme_count.toLocaleString()} programmes · ${s.last_synced.replace("T", " ")}` : "not synced yet")}
                    </div>
                  </div>
                  <button className="btn ghost" onClick={() => void ipc.deleteEpgSource(s.id).then(() => setSources((x) => x.filter((y) => y.id !== s.id)))}>
                    <Icon name="trash" size={15} />
                  </button>
                </div>
              ))}
              <div className="flex gap-2">
                <input className="input" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="http://…/xmltv.php?… or …/guide.xml.gz" spellCheck={false} />
                <button
                  className="btn primary"
                  disabled={!/^https?:\/\//i.test(url.trim())}
                  onClick={() =>
                    void ipc
                      .addEpgSource(p.id, url.trim())
                      .then(() => {
                        setUrl("");
                        pushToast({ level: "info", title: "Guide import started" });
                      })
                      .catch((e) => pushToast({ level: "error", title: "Could not add source", body: String(e) }))
                  }
                >
                  Add
                </button>
              </div>
            </div>
          )}
          {p.type === "xtream" && (
            <div style={{ color: "var(--text-faint)", fontSize: 12 }}>
              Xtream playlists use the panel's own <code>xmltv.php</code> guide; it refreshes with the playlist.
            </div>
          )}
        </>
      )}
    </section>
  );
}

// ---------- Parental ----------

function ParentalTab() {
  const parental = useApp((s) => s.parental);
  const refreshParental = useApp((s) => s.refreshParental);
  const [pin, setPin] = useState("");
  const [cur, setCur] = useState("");
  const [kw, setKw] = useState((parental?.keywords ?? []).join(", "));
  const [msg, setMsg] = useState<string | null>(null);

  const run = async (f: () => Promise<unknown>, ok: string) => {
    setMsg(null);
    try {
      await f();
      await refreshParental();
      setMsg(ok);
      setPin("");
      setCur("");
    } catch (e) {
      setMsg(String(e));
    }
  };

  return (
    <section className="grid grid-cols-2 gap-x-6 gap-y-4">
      <div className="flex flex-col gap-3">
        <div style={{ fontSize: 12.5 }}>
          Status:{" "}
          <b style={{ color: parental?.enabled ? (parental.unlocked ? "var(--warn)" : "var(--accent-2)") : "var(--text-faint)" }}>
            {!parental?.enabled ? "off" : parental.unlocked ? "PIN set · unlocked this session" : "PIN set · filter active"}
          </b>
        </div>
        {parental?.enabled && (
          <div className="field">
            <label>Current PIN</label>
            <input className="input" type="password" inputMode="numeric" value={cur} onChange={(e) => setCur(e.target.value.replace(/\D/g, ""))} />
          </div>
        )}
        <div className="field">
          <label>{parental?.enabled ? "New PIN" : "Set a PIN (4–12 digits)"}</label>
          <input className="input" type="password" inputMode="numeric" value={pin} onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))} />
        </div>
        <div className="flex gap-2">
          <button className="btn primary" onClick={() => void run(() => ipc.setParentalPin(pin, parental?.enabled ? cur : null), "PIN saved. Filter is active.")}>
            {parental?.enabled ? "Change PIN" : "Enable"}
          </button>
          {parental?.enabled && (
            <>
              <button className="btn" onClick={() => void run(() => (parental.unlocked ? ipc.lockParental() : ipc.unlockParental(cur)), parental.unlocked ? "Locked." : "Unlocked for this session.")}>
                {parental.unlocked ? "Lock now" : "Unlock"}
              </button>
              <button className="btn danger" onClick={() => void run(() => ipc.clearParentalPin(cur), "PIN removed.")}>
                Remove PIN
              </button>
            </>
          )}
        </div>
        {msg && <div style={{ fontSize: 12, color: /saved|Unlocked|Locked|removed|Keywords/.test(msg) ? "var(--accent-2)" : "var(--danger)" }}>{msg}</div>}
      </div>
      <div className="field">
        <label>Hidden keywords (channels, groups, titles and categories containing any of these disappear while locked)</label>
        <textarea className="input" rows={6} value={kw} onChange={(e) => setKw(e.target.value)} placeholder="xxx, adult, 18+" />
        <button
          className="btn"
          onClick={() =>
            void run(
              () =>
                ipc.setParentalKeywords(
                  kw.split(/[,\n]/).map((s) => s.trim()).filter(Boolean),
                  cur || null,
                ),
              "Keywords saved.",
            )
          }
        >
          Save keywords
        </button>
        <div style={{ color: "var(--text-faint)", fontSize: 11.5 }}>Matching is case-insensitive and applies to Live, Movies and Series. Unlocking lasts until the app restarts or you lock it.</div>
      </div>
    </section>
  );
}

// ---------- License ----------

function LicenseTab() {
  const license = useApp((s) => s.license);
  const refreshLicense = useApp((s) => s.refreshLicense);
  const [guid, setGuid] = useState("");
  const [key, setKey] = useState("");
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    ipc.getMachineGuid().then(setGuid).catch(() => {});
  }, []);
  const activate = async () => {
    setMsg(null);
    try {
      const st = await ipc.activateLicense({ license_key: key.trim(), machine_guid: guid });
      await refreshLicense();
      setMsg(`Activated: ${st.tier}`);
    } catch (e) {
      setMsg(String(e));
    }
  };
  return (
    <section className="flex flex-col gap-3" style={{ maxWidth: 520 }}>
      <div style={{ fontSize: 12.5 }}>
        Tier: <b>{license?.tier ?? "—"}</b>
        {license?.expires_at && license.tier === "trial" && <span style={{ color: "var(--text-faint)" }}> · trial ends {new Date(license.expires_at * 1000).toLocaleString()}</span>}
      </div>
      <div style={{ color: "var(--text-dim)", fontSize: 12, lineHeight: 1.5 }}>
        Trial: full PRO for 72 hours after your first import. Free afterwards: one playlist, live TV, trimmed guide. PRO (one-time): unlimited
        playlists and favorites, full guide, recording, downloads, backup, multiscreen. No subscription. No analytics.
      </div>
      <div className="field">
        <label>Machine id (send this to get a key)</label>
        <div className="flex gap-2">
          <input className="input" value={guid} readOnly style={{ fontFamily: "var(--mono)", fontSize: 11.5 }} />
          <button className="btn" onClick={() => void navigator.clipboard.writeText(guid)}>
            Copy
          </button>
        </div>
      </div>
      <div className="field">
        <label>License key</label>
        <div className="flex gap-2">
          <input className="input" value={key} onChange={(e) => setKey(e.target.value)} placeholder="DIPTV1.…" spellCheck={false} />
          <button className="btn primary" onClick={() => void activate()}>
            Activate
          </button>
        </div>
        {msg && <div style={{ fontSize: 12, color: msg.startsWith("Activated") ? "var(--accent-2)" : "var(--danger)" }}>{msg}</div>}
      </div>
    </section>
  );
}

function BackupPanel() {
  const pushToast = useApp((s) => s.pushToast);
  const reloadPlaylists = useApp((s) => s.reloadPlaylists);
  const [pass, setPass] = useState("");
  const [mode, setMode] = useState<"merge" | "replace">("merge");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const weak = pass.length < 6;

  const doExport = async () => {
    const path = await save({ defaultPath: `desktop-iptv-backup-${new Date().toISOString().slice(0, 10)}.diptvbk`, filters: [{ name: "desktop-iptv backup", extensions: ["diptvbk"] }] });
    if (!path) return;
    setBusy(true);
    try {
      const r = await ipc.backupExport(path, pass);
      setMsg(`Saved ${r.playlists} playlists, ${r.favorites} favorites, ${r.epg_overrides} EPG overrides (${(r.bytes / 1024).toFixed(1)} KB).`);
      pushToast({ level: "info", title: "Backup saved", body: r.path });
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  };
  const doImport = async () => {
    const chosen = await open({ multiple: false, directory: false, filters: [{ name: "desktop-iptv backup", extensions: ["diptvbk"] }] });
    if (typeof chosen !== "string") return;
    setBusy(true);
    try {
      const peek = await ipc.backupInspect(chosen, pass);
      const r = await ipc.backupImport(chosen, pass, mode === "replace");
      await reloadPlaylists();
      setMsg(`Restored ${r.playlists} of ${peek.playlists} playlists (${mode}); ${r.favorites} favorites and ${r.epg_overrides} EPG overrides will be applied as each playlist finishes syncing.`);
      pushToast({ level: "info", title: "Backup restored", body: "Playlists are syncing now. Set the parental PIN again if you used one." });
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="rounded-md p-3 flex flex-col gap-2" style={{ background: "var(--bg-elev-2)" }}>
      <div className="font-semibold">Encrypted backup</div>
      <div style={{ color: "var(--text-dim)", fontSize: 12 }}>
        Playlists (with their logins), favorites, EPG sources/offsets/overrides, settings, theme tokens and parental keywords — sealed with AES-256-GCM and a
        passphrase-derived key (Argon2id). The parental PIN and the license are machine-bound and are not included.
      </div>
      <div className="flex items-center gap-2">
        <input className="input" type="password" value={pass} onChange={(e) => setPass(e.target.value)} placeholder="Passphrase (6+ characters)" style={{ width: 260 }} autoComplete="off" />
        <button className="btn primary" disabled={weak || busy} onClick={() => void doExport()}>
          Export…
        </button>
        <div className="seg">
          <button className={mode === "merge" ? "on" : ""} onClick={() => setMode("merge")} title="Keep current playlists; skip duplicates">
            Merge
          </button>
          <button className={mode === "replace" ? "on" : ""} onClick={() => setMode("replace")} title="Delete current playlists first">
            Replace
          </button>
        </div>
        <button className="btn" disabled={weak || busy} onClick={() => void doImport()}>
          Import…
        </button>
      </div>
      {msg && <div style={{ fontSize: 12, color: msg.startsWith("Saved") || msg.startsWith("Restored") ? "var(--accent-2)" : "var(--danger)" }}>{msg}</div>}
    </div>
  );
}

function AboutTab() {
  const boot = useApp((s) => s.boot)!;
  return (
    <section className="flex flex-col gap-3" style={{ fontSize: 12.5, lineHeight: 1.55 }}>
      <BackupPanel />
      <div>{boot.legal_block}</div>
      <div style={{ fontFamily: "var(--mono)", fontSize: 11.5, color: "var(--text-dim)" }}>
        {boot.product} {boot.version} · {boot.platform}
        <br />
        engine: {boot.engine_description}
        <br />
        data: {boot.data_dir}
      </div>
      <div style={{ color: "var(--text-dim)" }}>
        <b>Does it buffer?</b> Buffering follows the stream, the source, the network, or the device. Change one variable. Open Diagnostics.
        <br />
        <b>Why am I behind the pub TV?</b> Provider HLS window. Low Latency shrinks <em>our</em> cache only. Switch server/CDN with your provider.
        <br />
        <b>Can I record?</b> PRO feature. Keep the app running. Use end-late for sports.
      </div>
    </section>
  );
}
