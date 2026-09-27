import { useEffect, useState } from "react";
import { ipc, type ConfigPayload, type HwDecoding } from "../../lib/ipc";
import { useApp } from "../../lib/store";

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

export default function SettingsDrawer() {
  const boot = useApp((s) => s.boot)!;
  const config = useApp((s) => s.config)!;
  const license = useApp((s) => s.license);
  const saveConfig = useApp((s) => s.saveConfig);
  const setUi = useApp((s) => s.setUi);
  const playlists = useApp((s) => s.playlists);
  const reloadPlaylists = useApp((s) => s.reloadPlaylists);
  const refreshLicense = useApp((s) => s.refreshLicense);
  const pushToast = useApp((s) => s.pushToast);
  const [draft, setDraft] = useState<ConfigPayload>(config);
  const [guid, setGuid] = useState("");
  const [key, setKey] = useState("");
  const [licMsg, setLicMsg] = useState<string | null>(null);

  useEffect(() => {
    ipc.getMachineGuid().then(setGuid).catch(() => {});
  }, []);

  const set = <K extends keyof ConfigPayload>(k: K, v: ConfigPayload[K]) => {
    const next = { ...draft, [k]: v };
    setDraft(next);
    void saveConfig({ [k]: v } as Partial<ConfigPayload>);
  };

  const close = () => setUi({ settingsOpen: false });

  const activate = async () => {
    setLicMsg(null);
    try {
      const st = await ipc.activateLicense({ license_key: key.trim(), machine_guid: guid });
      await refreshLicense();
      setLicMsg(`Activated: ${st.tier}`);
    } catch (e) {
      setLicMsg(String(e));
    }
  };

  const hwOptions = HWDEC.filter((h) => !h.os || h.os.includes(boot.platform));

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <div className="modal" style={{ width: "min(760px, 94vw)" }}>
        <div className="flex items-center justify-between mb-3">
          <div className="text-[16px] font-semibold">Settings</div>
          <button className="btn ghost" onClick={close}>
            ✕
          </button>
        </div>

        <div className="grid grid-cols-2 gap-x-6 gap-y-4">
          <section className="flex flex-col gap-3">
            <div className="font-semibold" style={{ color: "var(--text-dim)" }}>
              Playback
            </div>
            <div className="field">
              <label>Default profile</label>
              <div className="seg">
                <button className={draft.default_profile === "stable" ? "on" : ""} onClick={() => set("default_profile", "stable")}>
                  Stable (20–60 s cache)
                </button>
                <button
                  className={draft.default_profile === "low_latency" ? "on" : ""}
                  onClick={() => set("default_profile", "low_latency")}
                >
                  Low latency (3 s)
                </button>
              </div>
            </div>
            <div className="field">
              <label>Stable cache · {draft.stable_cache_secs} s</label>
              <input
                type="range"
                min={20}
                max={60}
                value={draft.stable_cache_secs}
                onChange={(e) => set("stable_cache_secs", Number(e.target.value))}
                style={{ accentColor: "var(--accent)" }}
              />
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
              <input
                type="range"
                min={100}
                max={130}
                value={draft.audio_boost}
                onChange={(e) => set("audio_boost", Number(e.target.value))}
                style={{ accentColor: "var(--accent)" }}
              />
            </div>
            <div className="field">
              <label>Audio delay · {draft.audio_delay_ms} ms</label>
              <input
                type="range"
                min={-1000}
                max={1000}
                step={10}
                value={draft.audio_delay_ms}
                onChange={(e) => set("audio_delay_ms", Number(e.target.value))}
                style={{ accentColor: "var(--accent)" }}
              />
            </div>
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={draft.auto_play_last} onChange={(e) => set("auto_play_last", e.target.checked)} />
              Auto-play last channel on launch
            </label>
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={draft.hud_enabled} onChange={(e) => set("hud_enabled", e.target.checked)} />
              Always show the stats HUD on the player
            </label>
          </section>

          <section className="flex flex-col gap-3">
            <div className="font-semibold" style={{ color: "var(--text-dim)" }}>
              Interface
            </div>
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

            <div className="font-semibold mt-2" style={{ color: "var(--text-dim)" }}>
              Playlists
            </div>
            <div className="flex flex-col gap-1">
              {playlists.length === 0 && <div style={{ color: "var(--text-faint)" }}>None yet.</div>}
              {playlists.map((p) => (
                <div key={p.id} className="flex items-center gap-2 rounded-md px-2 py-1" style={{ background: "var(--bg-elev-2)" }}>
                  <div className="min-w-0 flex-1">
                    <div className="truncate font-medium">{p.name}</div>
                    <div className="truncate" style={{ color: "var(--text-faint)", fontFamily: "var(--mono)", fontSize: 10.5 }}>
                      {p.base_url_redacted}
                    </div>
                  </div>
                  <span className="count" style={{ color: "var(--text-faint)", fontSize: 11 }}>
                    {p.channel_count.toLocaleString()}
                  </span>
                  <button className="btn ghost" title="Refresh" onClick={() => void ipc.refreshPlaylist(p.id)}>
                    ↻
                  </button>
                  <button
                    className="btn ghost"
                    title="Remove"
                    onClick={() =>
                      void ipc
                        .deletePlaylist(p.id)
                        .then(reloadPlaylists)
                        .then(() => pushToast({ level: "info", title: `Removed ${p.name}` }))
                    }
                  >
                    🗑
                  </button>
                </div>
              ))}
            </div>

            <div className="font-semibold mt-2" style={{ color: "var(--text-dim)" }}>
              License
            </div>
            <div style={{ fontSize: 12 }}>
              Tier: <b>{license?.tier ?? "—"}</b>
              {license?.expires_at && license.tier === "trial" && (
                <span style={{ color: "var(--text-faint)" }}> · trial ends {new Date(license.expires_at * 1000).toLocaleString()}</span>
              )}
            </div>
            <div className="field">
              <label>Machine id</label>
              <input className="input" value={guid} readOnly style={{ fontFamily: "var(--mono)", fontSize: 11 }} />
            </div>
            <div className="field">
              <label>License key</label>
              <div className="flex gap-2">
                <input className="input" value={key} onChange={(e) => setKey(e.target.value)} placeholder="DIPTV1.…" spellCheck={false} />
                <button className="btn" onClick={() => void activate()}>
                  Activate
                </button>
              </div>
              {licMsg && <div style={{ fontSize: 12, color: licMsg.startsWith("Activated") ? "var(--accent-2)" : "var(--danger)" }}>{licMsg}</div>}
            </div>
          </section>
        </div>

        <div className="mt-4 pt-3 border-t" style={{ borderColor: "var(--border)", color: "var(--text-faint)", fontSize: 11.5, lineHeight: 1.5 }}>
          <div>{boot.legal_block}</div>
          <div className="mt-1" style={{ fontFamily: "var(--mono)" }}>
            {boot.product} {boot.version} · engine: {boot.engine_description}
          </div>
        </div>
      </div>
    </div>
  );
}
