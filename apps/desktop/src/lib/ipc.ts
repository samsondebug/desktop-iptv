/**
 * Typed IPC — the TypeScript mirror of `crates/app-core/src/ipc.rs` (CLAUDE.md §4) plus the
 * command-layer shapes in `apps/desktop/src-tauri/src/commands/*.rs`.
 * Do not invent parallel shapes here; change the Rust struct first, then this file.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------- contracts (app-core) ----------

export type ProfileMode = "low_latency" | "stable";
export type HwDecoding =
  | "auto-safe"
  | "d3d11va"
  | "d3d11va-copy"
  | "videotoolbox"
  | "videotoolbox-copy"
  | "vaapi"
  | "vaapi-copy"
  | "no";

export interface ConfigPayload {
  app_theme: string;
  hw_decoding: HwDecoding | string;
  default_profile: ProfileMode;
  auto_play_last: boolean;
  max_multiscreen_instances: number;
  hide_vod_tabs: boolean;
  hide_brand_chrome: boolean;
  hud_enabled: boolean;
  audio_boost: number; // 100..=130
  audio_delay_ms: number;
  stable_cache_secs: number; // 20..=60
  legal_accepted: boolean;
  /** Experimental Stalker / MAC portal sources. */
  stalker_enabled: boolean;
}

export interface ValidateLicenseCommand {
  license_key: string;
  machine_guid: string;
}

export interface LicenseStateResponse {
  is_valid: boolean;
  expires_at: number | null;
  tier: "free" | "trial" | "pro_lifetime";
}

export interface FtsQueryRequest {
  query_string: string;
  playlist_id: number;
  limit: number;
  offset: number;
}

export interface ChannelRecord {
  id: number;
  playlist_id: number;
  source_id: string;
  name: string;
  normalized_name: string;
  group_title: string | null;
  logo: string | null;
  stream_url: string;
  tvg_id: string | null;
  catchup_days: number;
}

export interface LoadStreamCommand {
  stream_url: string;
  profile_mode: ProfileMode;
  audio_boost: number;
}

export interface EngineTelemetryEvent {
  active_profile: string;
  width: number;
  height: number;
  codec_name: string;
  bitrate_kbps: number;
  fps: number;
  dropped_frames: number;
  cache_duration_secs: number;
  is_underrun: boolean;
  zap_ms: number | null;
  time_pos_s: number;
  duration_s: number;
  paused: boolean;
}

export interface ListChannelsRequest {
  playlist_id: number;
  group_title: string | null;
  limit: number;
  offset: number;
}

export interface GroupSummary {
  group_title: string;
  channel_count: number;
}

export interface PlaylistSummary {
  id: number;
  type: "m3u" | "xtream" | "stalker";
  name: string;
  base_url_redacted: string;
  channel_count: number;
  created: string;
}

export interface PlaylistMeta {
  id: number;
  type: string;
  epg_offset_min: number;
  stream_format: "ts" | "m3u8";
  account_json: string | null;
  last_synced: string | null;
  last_error: string | null;
}

export interface XtreamAccount {
  username: string;
  status: string | null;
  exp_date: number | null;
  is_trial: boolean;
  active_cons: number | null;
  max_connections: number | null;
  created_at: number | null;
  allowed_output_formats: string[];
  server_url: string | null;
  timezone: string | null;
  server_time: number | null;
  auth: boolean;
}

export interface SyncStats {
  inserted: number;
  updated: number;
  skipped: number;
  groups: number;
  elapsed_ms: number;
  warnings: string[];
}

export interface RenderStateSignal {
  viewport_start_index: number;
  viewport_end_index: number;
  selected_category: string | null;
  active_channel_id: number | null;
}

// ---------- command-layer shapes ----------

export type SyncPhase = "account" | "live" | "epg" | "vod";

export interface ImportProgress {
  playlist_id: number;
  phase: SyncPhase;
  stage: "fetching" | "parsing" | "indexing" | "done" | "error";
  channels: number;
  bytes: number;
  message: string | null;
}

export interface ImportDone {
  playlist_id: number;
  phase: SyncPhase;
  ok: boolean;
  stats: SyncStats | null;
  error: string | null;
  error_kind: string | null;
  preview: string | null;
}

export interface ParentalStatus {
  enabled: boolean;
  unlocked: boolean;
  keywords: string[];
}

export interface Bootstrap {
  version: string;
  product: string;
  legal_block: string;
  config: ConfigPayload;
  license: LicenseStateResponse;
  playlists: PlaylistSummary[];
  engine_kind: "mpv" | "stub";
  engine_description: string;
  last_channel_id: number | null;
  platform: string;
  parental: ParentalStatus;
  data_dir: string;
}

export type AddPlaylistSource =
  | { kind: "m3u_url"; url: string; user_agent: string | null; epg_url: string | null }
  | { kind: "m3u_file"; path: string; epg_url: string | null }
  | { kind: "xtream"; base_url: string; username: string; password: string; stream_format: "ts" | "m3u8" | null; user_agent: string | null }
  | { kind: "stalker"; portal_url: string; mac: string; user_agent: string | null; epg_url: string | null };

export type PlaybackItem =
  | { kind: "none" }
  | { kind: "channel"; id: number }
  | { kind: "vod"; id: number }
  | { kind: "episode"; id: number; series_id: number }
  | { kind: "url" };

export interface PlaybackState {
  item: PlaybackItem;
  channel_id: number | null;
  stream_url_redacted: string | null;
  profile: ProfileMode;
  volume: number;
  muted: boolean;
  paused: boolean;
  engine_kind: "mpv" | "stub";
  is_vod: boolean;
}

export type EngineEvent =
  | { type: "telemetry"; [k: string]: unknown }
  | { type: "playback_started"; url_redacted: string; zap_ms: number }
  | { type: "end_file"; reason: string; error: string | null; url_redacted: string }
  | { type: "buffering"; active: boolean }
  | { type: "log"; level: string; text: string }
  | { type: "shutdown" };

/** `EngineEvent::Telemetry` is serialized as `{ type: "telemetry", ...EngineTelemetryEvent }`. */
export function telemetryOf(ev: EngineEvent): EngineTelemetryEvent | null {
  if (ev.type !== "telemetry") return null;
  const { type: _t, ...rest } = ev as unknown as { type: string } & EngineTelemetryEvent;
  return rest as EngineTelemetryEvent;
}

// EPG
export interface Programme {
  channel_tvg_id: string;
  start: number; // unix seconds, offset-adjusted
  stop: number;
  title: string;
  desc: string | null;
}
export interface EpgGridRequest {
  playlist_id: number;
  channel_ids: number[];
  from: number;
  to: number;
}
export interface EpgGridRow {
  channel_id: number;
  tvg_id: string | null;
  programmes: Programme[];
}
export interface NowNext {
  channel_id: number;
  tvg_id: string | null;
  now: Programme | null;
  next: Programme | null;
}
export interface EpgStats {
  programmes: number;
  channels_with_epg: number;
  min_start: number | null;
  max_stop: number | null;
  offset_min: number;
}
export interface EpgSource {
  id: number;
  playlist_id: number;
  url_redacted: string;
  enabled: boolean;
  last_synced: string | null;
  last_error: string | null;
  programme_count: number;
}

// VOD
export type VodKind = "movie" | "series";
export interface VodRecord {
  id: number;
  playlist_id: number;
  kind: VodKind;
  source_id: string;
  title: string;
  poster: string | null;
  backdrop: string | null;
  year: number | null;
  tmdb_id: number | null;
  category: string | null;
  description: string | null;
  rating: number | null;
  genre: string | null;
  duration_s: number | null;
  stream_url: string | null;
  container_ext: string | null;
  added: number | null;
  episodes_synced: string | null;
}
export interface EpisodeRecord {
  id: number;
  series_id: number;
  source_id: string | null;
  season: number;
  episode: number;
  title: string | null;
  stream_url: string;
  duration: number | null;
  poster: string | null;
  container_ext: string | null;
}
export interface VodGroup {
  category: string;
  count: number;
}
export interface ProgressRecord {
  item_type: "vod" | "episode" | "channel";
  item_id: number;
  position_s: number;
  duration_s: number | null;
  finished: boolean;
  updated: string;
}
export interface SeriesDetail {
  series: VodRecord;
  episodes: EpisodeRecord[];
  progress: ProgressRecord[];
  fetched_now: boolean;
}
export interface ContinueItem {
  progress: ProgressRecord;
  vod: VodRecord | null;
  episode: EpisodeRecord | null;
  series: VodRecord | null;
}
export interface ListVodRequest {
  playlist_id: number;
  kind: VodKind;
  category: string | null;
  sort: "added" | "title" | "year" | "rating" | null;
  limit: number;
  offset: number;
}

// DVR / windows
export interface RecordingRecord {
  id: number;
  channel_id: number;
  channel_name: string;
  title: string | null;
  start: number;
  stop: number;
  extra_end_s: number;
  path: string;
  status: "scheduled" | "recording" | "completed" | "failed";
  bytes: number;
  error: string | null;
}
export interface DownloadRecord {
  id: number;
  item_type: "vod" | "episode";
  item_id: number;
  title: string;
  path: string;
  bytes_done: number;
  bytes_total: number | null;
  status: "queued" | "downloading" | "paused" | "completed" | "failed";
  error: string | null;
}
export interface RecordStartResult {
  id: number;
  mode: "tap" | "raw" | "headless" | "running";
  path: string;
  warning: string | null;
}
export interface ConnectionBudget {
  in_use: number;
  max: number | null;
}
export type DvrEvent =
  | { type: "recording_started"; id: number; mode: string }
  | { type: "recording_progress"; id: number; bytes: number }
  | { type: "recording_stopped"; id: number; status: string; bytes: number; error: string | null }
  | { type: "download_progress"; id: number; bytes_done: number; bytes_total: number | null }
  | { type: "download_done"; id: number; status: string; error: string | null };
export interface PaneInfo {
  label: string;
  playing: boolean;
  channel_id: number | null;
  has_audio: boolean;
}

// ---------- commands ----------

export const ipc = {
  // dvr
  recordNow: (channelId: number, minutes: number, title: string | null) => invoke<RecordStartResult>("record_now", { channelId, minutes, title }),
  scheduleRecording: (channelId: number, start: number, stop: number, title: string | null, extraEndS: number | null) =>
    invoke<number>("schedule_recording", { channelId, start, stop, title, extraEndS }),
  stopRecording: (id: number) => invoke<void>("stop_recording", { id }),
  listRecordings: () => invoke<RecordingRecord[]>("list_recordings"),
  deleteRecording: (id: number, deleteFile: boolean) => invoke<void>("delete_recording", { id, deleteFile }),
  playRecording: (id: number) => invoke<PlaybackState>("play_recording", { id }),
  connectionBudget: (playlistId: number | null) => invoke<ConnectionBudget>("connection_budget", { playlistId }),
  downloadItem: (itemType: "vod" | "episode", itemId: number) => invoke<DownloadRecord>("download_item", { itemType, itemId }),
  listDownloads: () => invoke<DownloadRecord[]>("list_downloads"),
  pauseDownload: (id: number) => invoke<void>("pause_download", { id }),
  resumeDownload: (id: number) => invoke<void>("resume_download", { id }),
  deleteDownload: (id: number, deleteFile: boolean) => invoke<void>("delete_download", { id, deleteFile }),
  playDownload: (id: number) => invoke<PlaybackState>("play_download", { id }),
  mediaDir: () => invoke<string>("media_dir"),
  setMediaDir: (path: string | null) => invoke<void>("set_media_dir", { path }),

  // backup
  backupExport: (path: string, passphrase: string) => invoke<BackupSummary>("backup_export", { path, passphrase }),
  backupInspect: (path: string, passphrase: string) => invoke<BackupSummary>("backup_inspect", { path, passphrase }),
  backupImport: (path: string, passphrase: string, replace: boolean) => invoke<BackupSummary>("backup_import", { path, passphrase, replace }),

  // diagnostics
  diagHttpTrace: () => invoke<HttpTrace[]>("diag_http_trace"),
  diagClearTrace: () => invoke<void>("diag_clear_trace"),
  diagProbe: (url: string, profile: ProfileMode | null, timeoutSecs: number | null) => invoke<ProbeResult>("diag_probe", { url, profile, timeoutSecs }),
  diagCheckSource: (playlistId: number) => invoke<SourceCheck>("diag_check_source", { playlistId }),
  diagReport: () => invoke<string>("diag_report"),

  // windows
  setMiniMode: (on: boolean) => invoke<boolean>("set_mini_mode", { on }),
  isMiniMode: () => invoke<boolean>("is_mini_mode"),
  listPanes: () => invoke<PaneInfo[]>("list_panes"),
  paneOpen: (channelId: number | null) => invoke<PaneInfo>("pane_open", { channelId }),
  panePlay: (label: string, channelId: number) => invoke<void>("pane_play", { label, channelId }),
  paneStop: (label: string) => invoke<void>("pane_stop", { label }),
  paneClose: (label: string) => invoke<void>("pane_close", { label }),
  paneAudio: (label: string) => invoke<void>("pane_audio", { label }),
  paneSetVideoRect: (label: string, x: number, y: number, w: number, h: number, winW: number, winH: number) =>
    invoke<void>("pane_set_video_rect", { label, x, y, w, h, winW, winH }),
  paneTelemetry: (label: string) => invoke<EngineTelemetryEvent>("pane_telemetry", { label }),

  // bootstrap / config / license / theme
  getBootstrap: () => invoke<Bootstrap>("get_bootstrap"),
  getConfig: () => invoke<ConfigPayload>("get_config"),
  setConfig: (config: ConfigPayload) => invoke<ConfigPayload>("set_config", { config }),
  acceptLegal: () => invoke<ConfigPayload>("accept_legal"),
  getLicenseState: () => invoke<LicenseStateResponse>("get_license_state"),
  activateLicense: (cmd: ValidateLicenseCommand) => invoke<LicenseStateResponse>("activate_license", { cmd }),
  getMachineGuid: () => invoke<string>("get_machine_guid"),
  getThemeTokens: () => invoke<string | null>("get_theme_tokens"),
  setThemeTokens: (json: string | null) => invoke<void>("set_theme_tokens", { json }),

  // playlists / catalog
  addPlaylist: (name: string, source: AddPlaylistSource) => invoke<number>("add_playlist", { name, source }),
  refreshPlaylist: (playlistId: number) => invoke<boolean>("refresh_playlist", { playlistId }),
  deletePlaylist: (playlistId: number) => invoke<void>("delete_playlist", { playlistId }),
  renamePlaylist: (playlistId: number, name: string) => invoke<void>("rename_playlist", { playlistId, name }),
  listPlaylists: () => invoke<PlaylistSummary[]>("list_playlists"),
  playlistMeta: (playlistId: number) => invoke<PlaylistMeta>("playlist_meta", { playlistId }),
  setStreamFormat: (playlistId: number, format: "ts" | "m3u8") => invoke<void>("set_stream_format", { playlistId, format }),
  isSyncing: (playlistId: number) => invoke<boolean>("is_syncing", { playlistId }),
  listGroups: (playlistId: number) => invoke<GroupSummary[]>("list_groups", { playlistId }),
  listChannels: (req: ListChannelsRequest) => invoke<ChannelRecord[]>("list_channels", { req }),
  countChannels: (playlistId: number, groupTitle: string | null) => invoke<number>("count_channels", { playlistId, groupTitle }),
  searchChannels: (req: FtsQueryRequest) => invoke<ChannelRecord[]>("search_channels", { req }),
  getChannel: (channelId: number) => invoke<ChannelRecord>("get_channel", { channelId }),
  getFavorites: () => invoke<ChannelRecord[]>("get_favorites"),
  getFavoriteIds: () => invoke<number[]>("get_favorite_ids"),
  setFavorite: (channelId: number, on: boolean) => invoke<void>("set_favorite", { channelId, on }),
  getRecents: () => invoke<ChannelRecord[]>("get_recents"),

  // playback
  getPlaybackState: () => invoke<PlaybackState>("get_playback_state"),
  playChannel: (channelId: number) => invoke<PlaybackState>("play_channel", { channelId }),
  playVod: (vodId: number, fromStart: boolean) => invoke<PlaybackState>("play_vod", { vodId, fromStart }),
  playEpisode: (episodeId: number, fromStart: boolean) => invoke<PlaybackState>("play_episode", { episodeId, fromStart }),
  loadStream: (cmd: LoadStreamCommand) => invoke<PlaybackState>("load_stream", { cmd }),
  stopPlayback: () => invoke<PlaybackState>("stop_playback"),
  setProfile: (profile: ProfileMode) => invoke<PlaybackState>("set_profile", { profile }),
  setPause: (paused: boolean) => invoke<PlaybackState>("set_pause", { paused }),
  setMute: (muted: boolean) => invoke<PlaybackState>("set_mute", { muted }),
  setVolume: (volume: number) => invoke<PlaybackState>("set_volume", { volume }),
  seek: (secs: number) => invoke<void>("seek", { secs }),
  setVideoRect: (x: number, y: number, w: number, h: number, winW: number, winH: number) =>
    invoke<void>("set_video_rect", { x, y, w, h, winW, winH }),
  getTelemetry: () => invoke<EngineTelemetryEvent>("get_telemetry"),
  engineGetProperty: (name: string) => invoke<string | null>("engine_get_property", { name }),
  engineSetProperty: (name: string, value: string) => invoke<void>("engine_set_property", { name, value }),
  engineCommand: (args: string[]) => invoke<void>("engine_command", { args }),
  engineTracks: () => invoke<MpvTrack[]>("engine_tracks"),
  engineSelectTrack: (kind: "audio" | "sub" | "video", id: string) => invoke<void>("engine_select_track", { kind, id }),
  openInExternalPlayer: () => invoke<void>("open_in_external_player"),

  // epg
  epgGrid: (req: EpgGridRequest) => invoke<EpgGridRow[]>("epg_grid", { req }),
  epgNowNext: (channelId: number) => invoke<NowNext>("epg_now_next", { channelId }),
  epgStats: (playlistId: number) => invoke<EpgStats>("epg_stats", { playlistId }),
  setEpgOffset: (playlistId: number, minutes: number) => invoke<void>("set_epg_offset", { playlistId, minutes }),
  setEpgOverride: (channelId: number, tvgId: string | null) => invoke<void>("set_epg_override", { channelId, tvgId }),
  getEpgOverride: (channelId: number) => invoke<string | null>("get_epg_override", { channelId }),
  epgSearchIds: (playlistId: number, q: string) => invoke<string[]>("epg_search_ids", { playlistId, q }),
  listEpgSources: (playlistId: number) => invoke<EpgSource[]>("list_epg_sources", { playlistId }),
  addEpgSource: (playlistId: number, url: string) => invoke<number>("add_epg_source", { playlistId, url }),
  deleteEpgSource: (id: number) => invoke<void>("delete_epg_source", { id }),
  refreshEpg: (playlistId: number) => invoke<boolean>("refresh_epg", { playlistId }),

  // vod
  listVod: (req: ListVodRequest) => invoke<VodRecord[]>("list_vod", { req }),
  countVod: (playlistId: number, kind: VodKind, category: string | null) => invoke<number>("count_vod", { playlistId, kind, category }),
  vodGroups: (playlistId: number, kind: VodKind) => invoke<VodGroup[]>("vod_groups", { playlistId, kind }),
  searchVod: (playlistId: number, query: string, kind: VodKind | null, limit: number) =>
    invoke<VodRecord[]>("search_vod", { playlistId, query, kind, limit }),
  getVod: (vodId: number) => invoke<VodRecord>("get_vod", { vodId }),
  seriesDetail: (seriesId: number, force: boolean) => invoke<SeriesDetail>("series_detail", { seriesId, force }),
  continueWatching: () => invoke<ContinueItem[]>("continue_watching"),
  getProgress: (itemType: string, itemId: number) => invoke<ProgressRecord | null>("get_progress", { itemType, itemId }),
  clearProgress: (itemType: string, itemId: number) => invoke<void>("clear_progress", { itemType, itemId }),
  refreshVod: (playlistId: number) => invoke<boolean>("refresh_vod", { playlistId }),

  // parental
  parentalStatus: () => invoke<ParentalStatus>("parental_status"),
  setParentalPin: (pin: string, currentPin: string | null) => invoke<ParentalStatus>("set_parental_pin", { pin, currentPin }),
  clearParentalPin: (currentPin: string) => invoke<ParentalStatus>("clear_parental_pin", { currentPin }),
  unlockParental: (pin: string) => invoke<ParentalStatus>("unlock_parental", { pin }),
  lockParental: () => invoke<ParentalStatus>("lock_parental"),
  setParentalKeywords: (keywords: string[], pin: string | null) => invoke<ParentalStatus>("set_parental_keywords", { keywords, pin }),
};

export interface MpvTrack {
  id: number;
  type: "audio" | "sub" | "video";
  selected: boolean;
  title?: string;
  lang?: string;
  codec?: string;
  default?: boolean;
  forced?: boolean;
}

// ---------- backup ----------

export interface BackupSummary {
  path: string;
  bytes: number;
  playlists: number;
  favorites: number;
  epg_overrides: number;
  epg_sources: number;
}

// ---------- diagnostics ----------

export interface HttpTrace {
  id: number;
  at_ms: number;
  kind: string;
  method: string;
  url: string;
  status: number | null;
  elapsed_ms: number | null;
  content_type: string | null;
  content_length: number | null;
  preview: string | null;
  redirects: string[];
  error: string | null;
}

export interface ProbeResult {
  ok: boolean;
  url: string;
  profile: string;
  engine: string;
  elapsed_ms: number;
  ttff_ms: number | null;
  container: string | null;
  video_codec: string | null;
  audio_codec: string | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  hwdec: string | null;
  cache_secs: number | null;
  error: string | null;
  log: string[];
}

export interface AccountInfo {
  status: string | null;
  exp_date: number | null;
  max_connections: number | null;
  active_connections: number | null;
  server_timezone: string | null;
}

export interface SourceCheck {
  playlist_id: number;
  kind: string;
  ok: boolean;
  elapsed_ms: number;
  status: number | null;
  content_type: string | null;
  content_length: number | null;
  final_url: string | null;
  payload: string | null;
  preview: string | null;
  entries: number;
  entries_without_url: number;
  groups: number;
  epg_hint: string | null;
  account: AccountInfo | null;
  error: string | null;
}

// ---------- events ----------

export const events = {
  onEngine: (cb: (ev: EngineEvent) => void): Promise<UnlistenFn> => listen<EngineEvent>("engine_event", (e) => cb(e.payload)),
  onImportProgress: (cb: (ev: ImportProgress) => void): Promise<UnlistenFn> =>
    listen<ImportProgress>("import_progress", (e) => cb(e.payload)),
  onImportDone: (cb: (ev: ImportDone) => void): Promise<UnlistenFn> => listen<ImportDone>("import_done", (e) => cb(e.payload)),
  onPlaybackState: (cb: (ev: PlaybackState) => void): Promise<UnlistenFn> =>
    listen<PlaybackState>("playback_state", (e) => cb(e.payload)),
  onDvr: (cb: (ev: DvrEvent) => void): Promise<UnlistenFn> => listen<DvrEvent>("dvr_event", (e) => cb(e.payload)),
  onPanesChanged: (cb: (panes: PaneInfo[]) => void): Promise<UnlistenFn> => listen<PaneInfo[]>("panes_changed", (e) => cb(e.payload)),
  /** Favorites / overrides changed behind the UI's back (e.g. a restore finished applying). */
  onCatalogChanged: (cb: (playlistId: number) => void): Promise<UnlistenFn> => listen<number>("catalog_changed", (e) => cb(e.payload)),
  onPaneEngine: (cb: (label: string, ev: EngineEvent) => void): Promise<UnlistenFn> =>
    listen<{ label: string; event: EngineEvent }>("engine_event_pane", (e) => cb(e.payload.label, e.payload.event)),
};

export const isTauri = () =>
  typeof (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== "undefined";
