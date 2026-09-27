/**
 * Typed IPC — the TypeScript mirror of `crates/app-core/src/ipc.rs` (CLAUDE.md §4).
 * Do not invent parallel shapes here; change the Rust struct first, then this file.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------- contracts ----------

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
}

export interface ListChannelsRequest {
  playlist_id: number;
  group_title: string | null;
  limit: number;
  offset: number;
}

export interface ImportProgressEvent {
  playlist_id: number;
  stage: "fetching" | "parsing" | "indexing" | "done" | "error";
  channels: number;
  bytes: number;
  message: string | null;
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

// ---------- command-layer shapes (apps/desktop/src-tauri/src/commands.rs) ----------

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
}

export type AddPlaylistSource =
  | { kind: "m3u_url"; url: string; user_agent: string | null }
  | { kind: "m3u_file"; path: string };

export interface ImportDone {
  playlist_id: number;
  ok: boolean;
  stats: SyncStats | null;
  error: string | null;
  error_kind: string | null;
  preview: string | null;
}

export interface PlaybackState {
  channel_id: number | null;
  stream_url_redacted: string | null;
  profile: ProfileMode;
  volume: number;
  muted: boolean;
  paused: boolean;
  engine_kind: "mpv" | "stub";
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

// ---------- commands ----------

export const ipc = {
  getBootstrap: () => invoke<Bootstrap>("get_bootstrap"),
  getConfig: () => invoke<ConfigPayload>("get_config"),
  setConfig: (config: ConfigPayload) => invoke<ConfigPayload>("set_config", { config }),
  acceptLegal: () => invoke<ConfigPayload>("accept_legal"),

  addPlaylist: (name: string, source: AddPlaylistSource) => invoke<number>("add_playlist", { name, source }),
  refreshPlaylist: (playlistId: number) => invoke<void>("refresh_playlist", { playlistId }),
  deletePlaylist: (playlistId: number) => invoke<void>("delete_playlist", { playlistId }),
  listPlaylists: () => invoke<PlaylistSummary[]>("list_playlists"),

  listGroups: (playlistId: number) => invoke<GroupSummary[]>("list_groups", { playlistId }),
  listChannels: (req: ListChannelsRequest) => invoke<ChannelRecord[]>("list_channels", { req }),
  countChannels: (playlistId: number, groupTitle: string | null) =>
    invoke<number>("count_channels", { playlistId, groupTitle }),
  searchChannels: (req: FtsQueryRequest) => invoke<ChannelRecord[]>("search_channels", { req }),
  getChannel: (channelId: number) => invoke<ChannelRecord>("get_channel", { channelId }),
  getFavorites: () => invoke<ChannelRecord[]>("get_favorites"),
  getFavoriteIds: () => invoke<number[]>("get_favorite_ids"),
  setFavorite: (channelId: number, on: boolean) => invoke<void>("set_favorite", { channelId, on }),
  getRecents: () => invoke<ChannelRecord[]>("get_recents"),

  playChannel: (channelId: number) => invoke<PlaybackState>("play_channel", { channelId }),
  loadStream: (cmd: LoadStreamCommand) => invoke<PlaybackState>("load_stream", { cmd }),
  stopPlayback: () => invoke<PlaybackState>("stop_playback"),
  setProfile: (profile: ProfileMode) => invoke<PlaybackState>("set_profile", { profile }),
  setPause: (paused: boolean) => invoke<PlaybackState>("set_pause", { paused }),
  setMute: (muted: boolean) => invoke<PlaybackState>("set_mute", { muted }),
  setVolume: (volume: number) => invoke<PlaybackState>("set_volume", { volume }),
  setVideoRect: (x: number, y: number, w: number, h: number, winW: number, winH: number) =>
    invoke<void>("set_video_rect", { x, y, w, h, winW, winH }),
  getTelemetry: () => invoke<EngineTelemetryEvent>("get_telemetry"),
  engineGetProperty: (name: string) => invoke<string | null>("engine_get_property", { name }),
  engineSetProperty: (name: string, value: string) => invoke<void>("engine_set_property", { name, value }),
  engineCommand: (args: string[]) => invoke<void>("engine_command", { args }),

  getLicenseState: () => invoke<LicenseStateResponse>("get_license_state"),
  activateLicense: (cmd: ValidateLicenseCommand) => invoke<LicenseStateResponse>("activate_license", { cmd }),
  getMachineGuid: () => invoke<string>("get_machine_guid"),
};

// ---------- events ----------

export const events = {
  onEngine: (cb: (ev: EngineEvent) => void): Promise<UnlistenFn> =>
    listen<EngineEvent>("engine_event", (e) => cb(e.payload)),
  onImportProgress: (cb: (ev: ImportProgressEvent) => void): Promise<UnlistenFn> =>
    listen<ImportProgressEvent>("import_progress", (e) => cb(e.payload)),
  onImportDone: (cb: (ev: ImportDone) => void): Promise<UnlistenFn> =>
    listen<ImportDone>("import_done", (e) => cb(e.payload)),
};

export const isTauri = () => typeof (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== "undefined";
