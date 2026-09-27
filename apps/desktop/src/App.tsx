import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useApp } from "./lib/store";
import { useKeyboard } from "./lib/keys";
import TopBar from "./components/TopBar";
import Rail from "./components/Rail";
import LegalGate from "./components/LegalGate";
import Toasts from "./components/Toasts";
import PlayerPane from "./features/player/PlayerPane";
import ChannelList from "./features/live/ChannelList";
import NowPanel from "./features/live/NowPanel";
import SettingsDrawer from "./features/settings/SettingsDrawer";
import AddPlaylistDialog from "./features/settings/AddPlaylistDialog";
import DiagnosticsPanel from "./features/diagnostics/DiagnosticsPanel";

export default function App() {
  const ready = useApp((s) => s.ready);
  const init = useApp((s) => s.init);
  const config = useApp((s) => s.config);
  const ui = useApp((s) => s.ui);
  const playlists = useApp((s) => s.playlists);
  const theme = config?.app_theme ?? "dark";

  useEffect(() => {
    init().catch((e) => console.error("bootstrap failed", e));
  }, [init]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  // Keep the OS window and our layout in sync for fullscreen.
  useEffect(() => {
    getCurrentWindow()
      .setFullscreen(ui.fullscreen)
      .catch(() => {});
  }, [ui.fullscreen]);

  useKeyboard();

  if (!ready || !config) {
    return (
      <div className="opaque h-full w-full flex items-center justify-center" style={{ color: "var(--text-dim)" }}>
        Starting…
      </div>
    );
  }

  if (!config.legal_accepted) return <LegalGate />;

  const empty = playlists.length === 0;

  return (
    <div className="h-full w-full flex flex-col" style={{ background: ui.fullscreen ? "transparent" : "var(--bg)" }}>
      {!ui.fullscreen && <TopBar />}
      <div className="flex-1 min-h-0 flex">
        {!ui.fullscreen && <Rail />}
        <div className="flex-1 min-w-0 flex flex-col">
          <div className="flex-1 min-h-0 flex">
            <PlayerPane />
            {!ui.fullscreen && <NowPanel />}
          </div>
          {!ui.fullscreen && (
            <div className="opaque border-t" style={{ borderColor: "var(--border)", height: "42%" }}>
              {empty ? <EmptyState /> : <ChannelList />}
            </div>
          )}
        </div>
      </div>
      {ui.settingsOpen && <SettingsDrawer />}
      {ui.addPlaylistOpen && <AddPlaylistDialog />}
      {ui.diagnosticsOpen && <DiagnosticsPanel />}
      <Toasts />
    </div>
  );
}

function EmptyState() {
  const setUi = useApp((s) => s.setUi);
  return (
    <div className="h-full flex flex-col items-center justify-center gap-3 text-center px-6">
      <div className="text-[15px] font-semibold">No playlist yet</div>
      <div style={{ color: "var(--text-dim)", maxWidth: 520 }}>
        Add an M3U URL or file from your provider. This app does not include channels — you bring your own source.
      </div>
      <button className="btn primary" onClick={() => setUi({ addPlaylistOpen: true })}>
        Add playlist
      </button>
    </div>
  );
}
