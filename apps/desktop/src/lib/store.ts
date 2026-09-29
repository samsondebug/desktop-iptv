/**
 * App state. The catalog itself never lives here (CLAUDE.md non-negotiable 3): lists are
 * paged from SQLite on demand. This store holds selection, playback and UI state only.
 */
import { create } from "zustand";
import type { UpdateStatus } from "./updater";
import {
  events,
  ipc,
  telemetryOf,
  type Bootstrap,
  type ChannelRecord,
  type ConfigPayload,
  type EngineTelemetryEvent,
  type EpgStats,
  type EpisodeRecord,
  type GroupSummary,
  type ImportDone,
  type ImportProgress,
  type LicenseStateResponse,
  type PaneInfo,
  type ParentalStatus,
  type PlaybackState,
  type PlaylistMeta,
  type PlaylistSummary,
  type ProfileMode,
  type VodKind,
  type VodRecord,
} from "./ipc";

export type RailSelection =
  | { kind: "all" }
  | { kind: "favorites" }
  | { kind: "recents" }
  | { kind: "group"; title: string }
  | { kind: "continue" }
  /** Recordings & downloads, shown in the bottom pane like any other destination. */
  | { kind: "library" };

export type Tab = "live" | "movies" | "series";

export interface Toast {
  id: number;
  level: "info" | "warn" | "error";
  title: string;
  body?: string;
  sticky?: boolean;
  /** Optional primary action rendered as a button (the toast is dismissed after it runs). */
  action?: { label: string; onClick: () => void };
}

export interface ResumePrompt {
  kind: "vod" | "episode";
  id: number;
  title: string;
  position_s: number;
  duration_s: number | null;
  vod?: VodRecord;
  episode?: EpisodeRecord;
  series?: VodRecord | null;
}

interface AppStore {
  ready: boolean;
  boot: Bootstrap | null;
  config: ConfigPayload | null;
  license: LicenseStateResponse | null;
  parental: ParentalStatus | null;
  playlists: PlaylistSummary[];
  activePlaylistId: number | null;
  playlistMeta: PlaylistMeta | null;
  groups: GroupSummary[];
  epgStats: EpgStats | null;
  tab: Tab;
  rail: RailSelection;
  favoriteIds: Set<number>;
  favorites: ChannelRecord[];
  recents: ChannelRecord[];
  search: string;
  /** Bumps whenever the list contents may have changed (import done, favorites…). */
  listVersion: number;
  /** Bumps when EPG data changed. */
  epgVersion: number;
  guideMode: "list" | "guide";

  playback: PlaybackState | null;
  currentChannel: ChannelRecord | null;
  currentVod: VodRecord | null;
  currentEpisode: EpisodeRecord | null;
  currentSeries: VodRecord | null;
  telemetry: EngineTelemetryEvent | null;
  buffering: boolean;
  lastZapMs: number | null;
  lastError: string | null;
  engineLog: { level: string; text: string; at: number }[];

  imports: Record<number, ImportProgress>;
  toasts: Toast[];
  /** Self-updater state (lib/updater.ts). */
  update: UpdateStatus;
  panes: PaneInfo[];
  mini: boolean;
  /** recording id → bytes (live progress) */
  recordingBytes: Record<number, number>;
  activeRecordings: number;
  downloadProgress: Record<number, { done: number; total: number | null }>;

  ui: {
    fullscreen: boolean;
    settingsOpen: boolean;
    settingsTab: "playback" | "interface" | "playlists" | "guide" | "parental" | "license" | "about";
    addPlaylistOpen: boolean;
    diagnosticsOpen: boolean;
    selectedIndex: number;
    seriesOpen: number | null;
    resumePrompt: ResumePrompt | null;
    unlockOpen: boolean;
    epgEditChannel: ChannelRecord | null;
    vodSort: "added" | "title" | "year" | "rating";
    vodCategory: string | null;
    /** VOD filter chips (decade / rating). Empty object = everything. */
    vodFilter: import("./ipc").VodFilter;
    recordDialog: { channel: ChannelRecord; programme?: { start: number; stop: number; title: string } } | null;
    /** Digits typed for channel-number zap ("" when idle); rendered by ZapOverlay. */
    zapDigits: string;
    /** Channel being renamed in the rename modal. */
    renameChannel: ChannelRecord | null;
  };
  setMini: (on: boolean) => Promise<void>;
  openPane: (channelId: number | null) => Promise<void>;

  init: () => Promise<void>;
  reloadPlaylists: () => Promise<void>;
  selectPlaylist: (id: number | null) => Promise<void>;
  reloadPlaylistMeta: () => Promise<void>;
  reloadEpgStats: () => Promise<void>;
  setTab: (t: Tab) => void;
  selectRail: (sel: RailSelection) => void;
  setSearch: (q: string) => void;
  refreshSidebarLists: () => Promise<void>;
  toggleFavorite: (ch: ChannelRecord) => Promise<void>;
  renameChannel: (ch: ChannelRecord, name: string | null) => Promise<void>;
  hideChannel: (ch: ChannelRecord) => Promise<void>;
  hideGroup: (playlistId: number, groupTitle: string) => Promise<void>;

  play: (ch: ChannelRecord) => Promise<void>;
  playVod: (v: VodRecord, fromStart?: boolean) => Promise<void>;
  playEpisode: (e: EpisodeRecord, series: VodRecord | null, fromStart?: boolean) => Promise<void>;
  applyPlaybackState: (pb: PlaybackState) => Promise<void>;
  stop: () => Promise<void>;
  setProfile: (p: ProfileMode) => Promise<void>;
  toggleProfile: () => Promise<void>;
  togglePause: () => Promise<void>;
  toggleMute: () => Promise<void>;
  setVolume: (v: number) => Promise<void>;
  seekBy: (delta: number) => Promise<void>;

  saveConfig: (patch: Partial<ConfigPayload>) => Promise<void>;
  acceptLegal: () => Promise<void>;
  refreshLicense: () => Promise<void>;
  refreshParental: () => Promise<void>;

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
  parental: null,
  playlists: [],
  activePlaylistId: null,
  playlistMeta: null,
  groups: [],
  epgStats: null,
  tab: "live",
  rail: { kind: "all" },
  favoriteIds: new Set(),
  favorites: [],
  recents: [],
  search: "",
  listVersion: 0,
  epgVersion: 0,
  guideMode: "list",

  playback: null,
  currentChannel: null,
  currentVod: null,
  currentEpisode: null,
  currentSeries: null,
  telemetry: null,
  buffering: false,
  lastZapMs: null,
  lastError: null,
  engineLog: [],

  imports: {},
  toasts: [],
  update: { phase: "idle" },
  panes: [],
  mini: false,
  recordingBytes: {},
  activeRecordings: 0,
  downloadProgress: {},

  ui: {
    fullscreen: false,
    settingsOpen: false,
    settingsTab: "playback",
    addPlaylistOpen: false,
    diagnosticsOpen: false,
    selectedIndex: -1,
    seriesOpen: null,
    resumePrompt: null,
    unlockOpen: false,
    epgEditChannel: null,
    vodSort: "added",
    vodCategory: null,
    vodFilter: {},
    recordDialog: null,
    zapDigits: "",
    renameChannel: null,
  },

  init: async () => {
    const boot = await ipc.getBootstrap();
    set({ boot, config: boot.config, license: boot.license, playlists: boot.playlists, parental: boot.parental });
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
        case "end_file": {
          const live = !!get().playback && !get().playback?.is_vod && get().playback?.item.kind !== "none";
          if (live) {
            // The backend reconnects live streams on its own (see playback_notice); show the
            // spinner and keep the reason as the banner text until the first frame is back.
            set({ buffering: true, lastError: ev.error ? `Stream error: ${ev.error} — reconnecting…` : "Stream ended — reconnecting…" });
          } else if (ev.error) {
            set({ lastError: ev.error, buffering: false });
            get().pushToast({ level: "error", title: "Stream ended with an error", body: ev.error });
          } else {
            set({ buffering: false });
          }
          break;
        }
        case "log":
          set((s) => ({ engineLog: [...s.engineLog.slice(-199), { level: ev.level, text: ev.text, at: Date.now() }] }));
          break;
        case "shutdown":
          break;
      }
    });

    await events.onPlaybackState((pb) => {
      void get().applyPlaybackState(pb);
    });

    await events.onDvr((ev) => {
      switch (ev.type) {
        case "recording_started":
          set((s) => ({ activeRecordings: s.activeRecordings + 1 }));
          get().pushToast({ level: "info", title: ev.mode === "tap" ? "Recording (same connection as playback)" : ev.mode === "headless" ? "Recording (HLS, separate player)" : "Recording (separate connection)" });
          break;
        case "recording_progress":
          set((s) => ({ recordingBytes: { ...s.recordingBytes, [ev.id]: ev.bytes } }));
          break;
        case "recording_stopped":
          set((s) => {
            const recordingBytes = { ...s.recordingBytes };
            delete recordingBytes[ev.id];
            return { recordingBytes, activeRecordings: Math.max(0, s.activeRecordings - 1) };
          });
          get().pushToast({
            level: ev.status === "failed" ? "error" : "info",
            title: ev.status === "failed" ? "Recording failed" : `Recording saved (${(ev.bytes / 1e6).toFixed(0)} MB)`,
            body: ev.error ?? undefined,
          });
          break;
        case "download_progress":
          set((s) => ({ downloadProgress: { ...s.downloadProgress, [ev.id]: { done: ev.bytes_done, total: ev.bytes_total } } }));
          break;
        case "download_done":
          set((s) => {
            const downloadProgress = { ...s.downloadProgress };
            delete downloadProgress[ev.id];
            return { downloadProgress };
          });
          if (ev.status === "completed") get().pushToast({ level: "info", title: "Download finished" });
          else if (ev.status === "failed") get().pushToast({ level: "error", title: "Download failed", body: ev.error ?? undefined });
          break;
        case "reminder_fired": {
          const startsInMin = Math.max(0, Math.ceil((ev.start - Date.now() / 1000) / 60));
          get().pushToast({
            level: "info",
            sticky: true,
            title: `Reminder: ${ev.title}`,
            body: startsInMin > 0 ? `Starts in ${startsInMin} min on ${ev.channel_name}` : `Now on ${ev.channel_name}`,
            action: {
              label: "Watch",
              onClick: () => {
                void ipc.getChannel(ev.channel_id).then((ch) => get().play(ch)).catch(() => {});
              },
            },
          });
          break;
        }
      }
    });

    await events.onPanesChanged((panes) => set({ panes }));
    await events.onPlaybackNotice((n) => {
      if (n.attempt === 1 || n.attempt % 3 === 0) get().pushToast({ level: n.attempt >= 6 ? "error" : "info", title: "Reconnecting", body: n.message });
    });
    await events.onCatalogChanged(async () => {
      await get().refreshSidebarLists();
      set((s) => ({ listVersion: s.listVersion + 1 }));
    });
    ipc.listPanes().then((panes) => set({ panes })).catch(() => {});
    ipc.listRecordings().then((r) => set({ activeRecordings: r.filter((x) => x.status === "recording").length })).catch(() => {});

    await events.onImportProgress((ev) => {
      set((s) => ({ imports: { ...s.imports, [ev.playlist_id]: ev } }));
    });

    await events.onImportDone(async (ev: ImportDone) => {
      set((s) => {
        const imports = { ...s.imports };
        delete imports[ev.playlist_id];
        return { imports, listVersion: s.listVersion + 1, epgVersion: ev.phase === "epg" ? s.epgVersion + 1 : s.epgVersion };
      });
      await get().reloadPlaylists();
      await get().refreshLicense();
      const st = ev.stats;
      if (ev.ok && st) {
        if (ev.phase === "live") {
          get().pushToast({
            level: "info",
            title: `Imported ${(st.inserted + st.updated).toLocaleString()} channels in ${st.groups} groups`,
            body: `${(st.elapsed_ms / 1000).toFixed(1)} s${st.warnings.length ? " · " + st.warnings.join(" · ") : ""}`,
          });
          if (get().activePlaylistId === null || get().activePlaylistId === ev.playlist_id) await get().selectPlaylist(ev.playlist_id);
        } else if (ev.phase === "epg") {
          get().pushToast({
            level: "info",
            title: `Guide: ${st.inserted.toLocaleString()} programmes for ${st.groups} channels`,
            body: st.warnings.slice(0, 3).join(" · ") || undefined,
          });
          if (get().activePlaylistId === ev.playlist_id) {
            await get().reloadEpgStats();
            if ((get().epgStats?.programmes ?? 0) > 0) set({ guideMode: "guide" });
          }
        } else if (ev.phase === "vod") {
          get().pushToast({ level: "info", title: `Library: ${st.inserted.toLocaleString()} new titles`, body: st.warnings.slice(0, 2).join(" · ") || undefined });
        }
      } else if (!ev.ok) {
        get().pushToast({
          level: ev.phase === "epg" ? "warn" : "error",
          title:
            ev.error_kind === "html"
              ? `That URL returned a web page, not a ${ev.phase === "epg" ? "guide" : "playlist"}`
              : ev.phase === "account"
                ? "Provider login failed"
                : ev.phase === "epg"
                  ? "Guide import failed"
                  : ev.phase === "vod"
                    ? "Library import failed"
                    : "Import failed",
          body: [ev.error, ev.preview ? `First bytes: ${ev.preview.slice(0, 160)}` : null].filter(Boolean).join("\n"),
          sticky: ev.phase !== "epg",
        });
      }
      await get().reloadPlaylistMeta();
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
    set((s) => ({ activePlaylistId: id, rail: { kind: "all" }, search: "", ui: { ...s.ui, selectedIndex: -1, vodCategory: null } }));
    set({ groups: id != null ? await ipc.listGroups(id) : [] });
    await get().reloadPlaylistMeta();
    await get().reloadEpgStats();
    set((s) => ({ listVersion: s.listVersion + 1, guideMode: (s.epgStats?.programmes ?? 0) > 0 ? "guide" : "list" }));
  },

  reloadPlaylistMeta: async () => {
    const id = get().activePlaylistId;
    set({ playlistMeta: id != null ? await ipc.playlistMeta(id).catch(() => null) : null });
  },

  reloadEpgStats: async () => {
    const id = get().activePlaylistId;
    set({ epgStats: id != null ? await ipc.epgStats(id).catch(() => null) : null });
  },

  setTab: (t) => set((s) => ({ tab: t, search: "", rail: t === "live" ? s.rail : { kind: "all" }, ui: { ...s.ui, selectedIndex: -1 } })),
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

  renameChannel: async (ch, name) => {
    try {
      await ipc.renameChannel(ch.id, name);
      set((s) => ({ listVersion: s.listVersion + 1 }));
      await get().refreshSidebarLists();
    } catch (e) {
      get().pushToast({ level: "error", title: "Rename failed", body: String(e) });
    }
  },

  hideChannel: async (ch) => {
    try {
      await ipc.setChannelHidden(ch.id, true);
      set((s) => ({ listVersion: s.listVersion + 1 }));
      await get().refreshSidebarLists();
      get().pushToast({
        level: "info",
        title: `Hidden: ${ch.name}`,
        body: "Unhide any time in Settings → Playlists.",
        action: {
          label: "Undo",
          onClick: () => {
            void ipc.setChannelHidden(ch.id, false).then(() => set((s) => ({ listVersion: s.listVersion + 1 })));
          },
        },
      });
    } catch (e) {
      get().pushToast({ level: "error", title: "Could not hide channel", body: String(e) });
    }
  },

  hideGroup: async (playlistId, groupTitle) => {
    try {
      await ipc.setGroupHidden(playlistId, groupTitle, true);
      const groups = await ipc.listGroups(playlistId);
      set((s) => ({
        groups,
        listVersion: s.listVersion + 1,
        // Leave a rail pointing at the now-hidden group.
        rail: s.rail.kind === "group" && s.rail.title === groupTitle ? { kind: "all" } : s.rail,
      }));
      await get().refreshSidebarLists();
      get().pushToast({
        level: "info",
        title: `Hidden group: ${groupTitle || "(no group)"}`,
        body: "Unhide any time in Settings → Playlists.",
        action: {
          label: "Undo",
          onClick: () => {
            void ipc.setGroupHidden(playlistId, groupTitle, false).then(async () => {
              set((s) => ({ listVersion: s.listVersion + 1 }));
              set({ groups: await ipc.listGroups(playlistId) });
            });
          },
        },
      });
    } catch (e) {
      get().pushToast({ level: "error", title: "Could not hide group", body: String(e) });
    }
  },

  play: async (ch) => {
    set({ currentChannel: ch, currentVod: null, currentEpisode: null, currentSeries: null, lastError: null, lastZapMs: null, buffering: true });
    try {
      const playback = await ipc.playChannel(ch.id);
      set({ playback });
      set({ recents: await ipc.getRecents() });
    } catch (e) {
      set({ buffering: false, lastError: String(e) });
      get().pushToast({ level: "error", title: "Could not start playback", body: String(e) });
    }
  },

  playVod: async (v, fromStart = false) => {
    if (!fromStart) {
      const p = await ipc.getProgress("vod", v.id).catch(() => null);
      if (p && !p.finished && p.position_s > 30) {
        get().setUi({ resumePrompt: { kind: "vod", id: v.id, title: v.title, position_s: p.position_s, duration_s: p.duration_s ?? v.duration_s, vod: v } });
        return;
      }
    }
    set({ currentVod: v, currentChannel: null, currentEpisode: null, currentSeries: null, lastError: null, lastZapMs: null, buffering: true });
    try {
      set({ playback: await ipc.playVod(v.id, fromStart) });
    } catch (e) {
      set({ buffering: false, lastError: String(e) });
      get().pushToast({ level: "error", title: "Could not start playback", body: String(e) });
    }
  },

  playEpisode: async (e, series, fromStart = false) => {
    if (!fromStart) {
      const p = await ipc.getProgress("episode", e.id).catch(() => null);
      if (p && !p.finished && p.position_s > 30) {
        get().setUi({
          resumePrompt: {
            kind: "episode",
            id: e.id,
            title: `${series?.title ?? "Series"} · S${e.season}E${e.episode}${e.title ? " · " + e.title : ""}`,
            position_s: p.position_s,
            duration_s: p.duration_s ?? e.duration,
            episode: e,
            series,
          },
        });
        return;
      }
    }
    set({ currentEpisode: e, currentSeries: series, currentVod: null, currentChannel: null, lastError: null, lastZapMs: null, buffering: true });
    try {
      set({ playback: await ipc.playEpisode(e.id, fromStart) });
    } catch (err) {
      set({ buffering: false, lastError: String(err) });
      get().pushToast({ level: "error", title: "Could not start playback", body: String(err) });
    }
  },

  /** Backend changed playback (auto-next). Resolve what is playing for the UI. */
  applyPlaybackState: async (pb) => {
    set({ playback: pb });
    const item = pb.item;
    if (item.kind === "episode") {
      const cur = get().currentEpisode;
      if (cur?.id !== item.id) {
        try {
          const detail = await ipc.seriesDetail(item.series_id, false);
          const ep = detail.episodes.find((x) => x.id === item.id) ?? null;
          set({ currentEpisode: ep, currentSeries: detail.series, currentVod: null, currentChannel: null, lastZapMs: null });
          if (ep) get().pushToast({ level: "info", title: `Up next: S${ep.season}E${ep.episode}${ep.title ? " · " + ep.title : ""}` });
        } catch {
          /* ignore */
        }
      }
    }
  },

  stop: async () => {
    const playback = await ipc.stopPlayback();
    set({ playback, currentChannel: null, currentVod: null, currentEpisode: null, currentSeries: null, telemetry: null, buffering: false });
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
  seekBy: async (delta) => {
    const t = get().telemetry;
    if (!get().playback?.is_vod || !t) return;
    await ipc.seek(Math.max(0, t.time_pos_s + delta)).catch(() => {});
  },

  saveConfig: async (patch) => {
    const cur = get().config;
    if (!cur) return;
    set({ config: await ipc.setConfig({ ...cur, ...patch }) });
  },
  acceptLegal: async () => set({ config: await ipc.acceptLegal() }),
  refreshLicense: async () => set({ license: await ipc.getLicenseState() }),
  refreshParental: async () => {
    const parental = await ipc.parentalStatus();
    set((s) => ({ parental, listVersion: s.listVersion + 1 }));
    const id = get().activePlaylistId;
    if (id != null) set({ groups: await ipc.listGroups(id) });
  },

  setMini: async (on) => {
    try {
      const v = await ipc.setMiniMode(on);
      set({ mini: v });
    } catch (e) {
      get().pushToast({ level: "error", title: "Mini mode failed", body: String(e) });
    }
  },
  openPane: async (channelId) => {
    try {
      const budget = await ipc.connectionBudget(get().activePlaylistId);
      if (budget.max != null && budget.in_use + 1 > budget.max) {
        get().pushToast({
          level: "warn",
          title: `Provider allows ${budget.max} connection${budget.max === 1 ? "" : "s"}; ${budget.in_use} in use`,
          body: "Opening another pane may get one of the streams refused. Continuing anyway.",
        });
      }
      const pane = await ipc.paneOpen(channelId);
      set({ panes: await ipc.listPanes() });
      get().pushToast({ level: "info", title: `Opened ${pane.label}`, body: "Audio stays on the main player — use the speaker button in the pane to switch." });
    } catch (e) {
      get().pushToast({ level: "error", title: "Could not open a pane", body: String(e) });
    }
  },

  setUi: (patch) => set((s) => ({ ui: { ...s.ui, ...patch } })),
  pushToast: (t) => {
    const id = toastSeq++;
    set((s) => ({ toasts: [...s.toasts.slice(-4), { ...t, id }] }));
    if (!t.sticky) setTimeout(() => get().dismissToast(id), 6000);
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

export function fmtTime(unix: number): string {
  return new Date(unix * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function fmtDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${String(sec).padStart(2, "0")}` : `${m}:${String(sec).padStart(2, "0")}`;
}

export function vodKindOfTab(tab: Tab): VodKind | null {
  return tab === "movies" ? "movie" : tab === "series" ? "series" : null;
}
