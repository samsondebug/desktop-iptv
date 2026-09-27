import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ipc } from "./lib/ipc";
import { useApp } from "./lib/store";
import { useKeyboard } from "./lib/keys";
import TopBar from "./components/TopBar";
import Rail from "./components/Rail";
import LegalGate from "./components/LegalGate";
import Toasts from "./components/Toasts";
import { EpgEditModal, ResumePromptModal, UnlockModal } from "./components/Modals";
import PlayerPane from "./features/player/PlayerPane";
import ChannelList from "./features/live/ChannelList";
import NowPanel from "./features/live/NowPanel";
import EpgGrid from "./features/epg/EpgGrid";
import VodBrowser from "./features/vod/VodBrowser";
import SeriesModal from "./features/vod/SeriesModal";
import SettingsDrawer from "./features/settings/SettingsDrawer";
import AddPlaylistDialog from "./features/settings/AddPlaylistDialog";
import DiagnosticsPanel from "./features/diagnostics/DiagnosticsPanel";

/** Apply user theme tokens (CSS variables) on top of the built-in theme. */
function useThemeTokens() {
  const config = useApp((s) => s.config);
  const theme = config?.app_theme ?? "dark";
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => {
    let cancelled = false;
    ipc
      .getThemeTokens()
      .then((json) => {
        if (cancelled) return;
        const root = document.documentElement;
        // Reset previously applied inline tokens.
        for (const name of Array.from(root.style)) if (name.startsWith("--")) root.style.removeProperty(name);
        if (!json) return;
        try {
          const obj = JSON.parse(json) as Record<string, string>;
          for (const [k, v] of Object.entries(obj)) if (k.startsWith("--")) root.style.setProperty(k, v);
        } catch {
          /* ignore */
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [config]);
}

export default function App() {
  const ready = useApp((s) => s.ready);
  const init = useApp((s) => s.init);
  const config = useApp((s) => s.config);
  const ui = useApp((s) => s.ui);
  const tab = useApp((s) => s.tab);
  const guideMode = useApp((s) => s.guideMode);
  const playlists = useApp((s) => s.playlists);

  useEffect(() => {
    init().catch((e) => console.error("bootstrap failed", e));
  }, [init]);

  useThemeTokens();

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
  const bottom = empty ? <EmptyState /> : tab === "live" ? guideMode === "guide" ? <EpgGrid /> : <ChannelList /> : <VodBrowser kind={tab === "movies" ? "movie" : "series"} />;

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
            <div className="opaque border-t" style={{ borderColor: "var(--border)", height: tab === "live" ? "44%" : "52%" }}>
              {bottom}
            </div>
          )}
        </div>
      </div>
      {ui.settingsOpen && <SettingsDrawer />}
      {ui.addPlaylistOpen && <AddPlaylistDialog />}
      {ui.diagnosticsOpen && <DiagnosticsPanel />}
      {ui.seriesOpen != null && <SeriesModal seriesId={ui.seriesOpen} />}
      {ui.resumePrompt && <ResumePromptModal />}
      {ui.unlockOpen && <UnlockModal />}
      {ui.epgEditChannel && <EpgEditModal />}
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
        Add your provider's Xtream login or an M3U URL/file. This app does not include channels — you bring your own source.
      </div>
      <button className="btn primary" onClick={() => setUi({ addPlaylistOpen: true })}>
        Add playlist
      </button>
    </div>
  );
}
