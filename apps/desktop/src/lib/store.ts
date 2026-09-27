/**
 * App state. The catalog itself never lives here (CLAUDE.md non-negotiable 3): lists are
 * paged from SQLite on demand. This store holds selection, playback and UI state only.
 */
import { create } from "zustand";
import {
  events,
  ipc,
  telemetryOf,
  type Bootstrap,
  type ChannelRecord,
  type ConfigPayload,
  type EngineTelemetryEvent,
  type GroupSummary,
  type ImportDone,
  type ImportProgressEvent,
  type LicenseStateResponse,
  type PlaybackState,
  type PlaylistSummary,
  type ProfileMode,
} from "./ipc";

export type RailSelection = { kind: "all" } | { kind: "favorites" } | { kind: "recents" } | { kind: "group"; title: string };

export interface Toast {
  id: number;
  level: "info" | "warn" | "error";
  title: string;
  body?: string;
  sticky?: boolean;
}

interface AppStore {
  ready: boolean;
  boot: Bootstrap | null;
  config: ConfigPayload | null;
  license: LicenseStateResponse | null;
  playlists: PlaylistSummary[];
  activePlaylistId: number | null;
  groups: GroupSummary[];
  rail: RailSelection;
  favoriteIds: Set<number>;
  favorites: ChannelRecord[];
  recents: ChannelRecord[];
  search: string;
  /** Bumps whenever the list contents may have changed (import done, favorites…). */
  listVersion: number;

  playback: PlaybackState | null;
  currentChannel: ChannelRecord | null;
  telemetry: EngineTelemetryEvent | null;
  buffering: boolean;
  lastZapMs: number | null;
  lastError: string | null;
  engineLog: { level: string; text: string; at: number }[];

  imports: Record<number, ImportProgressEvent>;
  toasts: Toast[];

  ui: {
    fullscreen: boolean;
    settingsOpen: boolean;
    addPlaylistOpen: boolean;
    diagnosticsOpen: boolean;
    selectedIndex: number;
  };

  init: () => Promise<void>;
  reloadPlaylists: () => Promise<void>;
  selectPlaylist: (id: number | null) => Promise<void>;
  selectRail: (sel: RailSelection) => void;
  setSearch: (q: string) => void;
  refreshSidebarLists: () => Promise<void>;
  toggleFavorite: (ch: ChannelRecord) => Promise<void>;

  play: (ch: ChannelRecord) => Promise<void>;
  stop: () => Promise<void>;
  setProfile: (p: ProfileMode) => Promise<void>;
  toggleProfile: () => Promise<void>;
  togglePause: () => Promise<void>;
  toggleMute: () => Promise<void>;
  setVolume: (v: number) => Promise<void>;

  saveConfig: (patch: Partial<ConfigPayload>) => Promise<void>;
  acceptLegal: () => Promise<void>;
  refreshLicense: () => Promise<void>;

  setUi: (patch: Partial<AppStore["ui"]>) => void;
  pushToast: (t: Omit<Toast, "id">) => void;
  dismissToast: (id: number) => void;
}

let toastSeq = 1;

export const useApp = create<AppStore>((set, get) => ({
  ready: false,
  boot: null,
  config: null,
  license: null,
  playlists: [],
  activePlaylistId: null,
  groups: [],
  rail: { kind: "all" },
  favoriteIds: new Set(),
  favorites: [],
  recents: [],
  search: "",
  listVersion: 0,

  playback: null,
  currentChannel: null,
  telemetry: null,
  buffering: false,
  lastZapMs: null,
  lastError: null,
  engineLog: [],

  imports: {},
  toasts: [],

  ui: { fullscreen: false, settingsOpen: false, addPlaylistOpen: false, diagnosticsOpen: false, selectedIndex: -1 },

  init: async () => {
    const boot = await ipc.getBootstrap();
    set({ boot, config: boot.config, license: boot.license, playlists: boot.playlists });
    const first = boot.playlists[0]?.id ?? null;
    await get().selectPlaylist(first);
    await get().refreshSidebarLists();

    await events.onEngine((ev) => {
      const t = telemetryOf(ev);
      if (t) {
        set({ telemetry: t, buffering: t.is_underrun });
        return;
      }
      switch (ev.type) {
        case "playback_started":
          set({ lastZapMs: ev.zap_ms, buffering: false, lastError: null });
          break;
        case "buffering":
          set({ buffering: ev.active });
          break;
        case "end_file":
          if (ev.error) {
            set({ lastError: ev.error });
            get().pushToast({ level: "error", title: "Stream ended with an error", body: ev.error });
          } else if (ev.reason === "eof") {
            get().pushToast({ level: "info", title: "Stream ended", body: "The provider closed the stream." });
          }
          break;
        case "log":
          set((s) => ({ engineLog: [...s.engineLog.slice(-199), { level: ev.level, text: ev.text, at: Date.now() }] }));
          break;
        case "shutdown":
          break;
      }
    });

    await events.onImportProgress((ev) => {
      set((s) => ({ imports: { ...s.imports, [ev.playlist_id]: ev } }));
    });

    await events.onImportDone(async (ev: ImportDone) => {
      set((s) => {
        const imports = { ...s.imports };
        delete imports[ev.playlist_id];
        return { imports, listVersion: s.listVersion + 1 };
      });
      await get().reloadPlaylists();
      await get().refreshLicense();
      if (ev.ok && ev.stats) {
        const st = ev.stats;
        get().pushToast({
          level: "info",
          title: `Imported ${st.inserted + st.updated} channels in ${st.groups} groups`,
          body: `${(st.elapsed_ms / 1000).toFixed(1)} s${st.warnings.length ? " · " + st.warnings.join(" · ") : ""}`,
        });
        if (get().activePlaylistId === null || get().activePlaylistId === ev.playlist_id) {
          await get().selectPlaylist(ev.playlist_id);
        }
      } else {
        get().pushToast({
          level: "error",
          title: ev.error_kind === "html" ? "That URL returned a web page, not a playlist" : "Import failed",
          body: [ev.error, ev.preview ? `First bytes: ${ev.preview.slice(0, 160)}` : null].filter(Boolean).join("\n"),
          sticky: true,
        });
      }
    });

    set({ ready: true });

    // Cable-box feel: resume the last channel.
    if (boot.config.auto_play_last && boot.last_channel_id != null && boot.config.legal_accepted) {
      try {
        const ch = await ipc.getChannel(boot.last_channel_id);
        await get().play(ch);
      } catch {
        /* channel may have been deleted */
      }
    }
  },

  reloadPlaylists: async () => {
    const playlists = await ipc.listPlaylists();
    set({ playlists });
    const { activePlaylistId } = get();
    if (activePlaylistId != null && !playlists.some((p) => p.id === activePlaylistId)) {
      await get().selectPlaylist(playlists[0]?.id ?? null);
    } else if (activePlaylistId != null) {
      set({ groups: await ipc.listGroups(activePlaylistId) });
    }
  },

  selectPlaylist: async (id) => {
    set({ activePlaylistId: id, rail: { kind: "all" }, search: "", ui: { ...get().ui, selectedIndex: -1 } });
    set({ groups: id != null ? await ipc.listGroups(id) : [] });
    set((s) => ({ listVersion: s.listVersion + 1 }));
  },

  selectRail: (sel) => set((s) => ({ rail: sel, search: "", ui: { ...s.ui, selectedIndex: -1 } })),
  setSearch: (q) => set((s) => ({ search: q, ui: { ...s.ui, selectedIndex: -1 } })),

  refreshSidebarLists: async () => {
    const [ids, favorites, recents] = await Promise.all([ipc.getFavoriteIds(), ipc.getFavorites(), ipc.getRecents()]);
    set({ favoriteIds: new Set(ids), favorites, recents });
  },

  toggleFavorite: async (ch) => {
    const on = !get().favoriteIds.has(ch.id);
    await ipc.setFavorite(ch.id, on);
    await get().refreshSidebarLists();
    if (get().rail.kind === "favorites") set((s) => ({ listVersion: s.listVersion + 1 }));
  },

  play: async (ch) => {
    set({ currentChannel: ch, lastError: null, lastZapMs: null, buffering: true });
    try {
      const playback = await ipc.playChannel(ch.id);
      set({ playback });
      const recents = await ipc.getRecents();
      set({ recents });
    } catch (e) {
      set({ buffering: false, lastError: String(e) });
      get().pushToast({ level: "error", title: "Could not start playback", body: String(e) });
    }
  },

  stop: async () => {
    const playback = await ipc.stopPlayback();
    set({ playback, currentChannel: null, telemetry: null, buffering: false });
  },

  setProfile: async (p) => {
    const playback = await ipc.setProfile(p);
    set({ playback, lastZapMs: null });
  },
  toggleProfile: async () => {
    const cur = get().playback?.profile ?? get().config?.default_profile ?? "stable";
    await get().setProfile(cur === "stable" ? "low_latency" : "stable");
  },
  togglePause: async () => {
    const pb = get().playback;
    if (!pb) return;
    set({ playback: await ipc.setPause(!pb.paused) });
  },
  toggleMute: async () => {
    const pb = get().playback;
    const muted = pb ? !pb.muted : true;
    set({ playback: await ipc.setMute(muted) });
  },
  setVolume: async (v) => {
    set({ playback: await ipc.setVolume(Math.max(0, Math.min(130, Math.round(v)))) });
  },

  saveConfig: async (patch) => {
    const cur = get().config;
    if (!cur) return;
    const saved = await ipc.setConfig({ ...cur, ...patch });
    set({ config: saved });
  },
  acceptLegal: async () => {
    set({ config: await ipc.acceptLegal() });
  },
  refreshLicense: async () => set({ license: await ipc.getLicenseState() }),

  setUi: (patch) => set((s) => ({ ui: { ...s.ui, ...patch } })),
  pushToast: (t) => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts.slice(-4), { ...t, id }] }));
    if (!t.sticky) setTimeout(() => get().dismissToast(id), 6000);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));
