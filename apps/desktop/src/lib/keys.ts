/**
 * Global keyboard shortcuts (CLAUDE.md §8):
 *   /  search     j/k or ↑/↓ rows      Enter play      f fullscreen     m mute
 *   space pause   p profile            ←/→ EPG window (live) or seek ±10 s (VOD)
 *   Shift+S settings   Shift+D diagnostics   1/2/3 tabs (Movies/Series)   Esc close/exit
 *   Digits on the Live tab type a channel number (TiviMate-style zap): Enter or a short
 *   pause plays that row of the current list; Backspace edits, Esc cancels.
 * List navigation is delegated to the mounted list via DOM events so the list owns its data.
 */
import { useEffect } from "react";
import { useApp } from "./store";

export const LIST_MOVE = "diptv:list-move";
export const LIST_ENTER = "diptv:list-enter";
export const EPG_SHIFT = "diptv:epg-shift";
export const ZAP_COMMIT = "diptv:zap";

/** How long after the last digit a zap commits on its own. */
const ZAP_TIMEOUT_MS = 1400;
let zapTimer: number | undefined;

function commitZap() {
  window.clearTimeout(zapTimer);
  const s = useApp.getState();
  const n = parseInt(s.ui.zapDigits, 10);
  s.setUi({ zapDigits: "" });
  if (Number.isFinite(n) && n > 0) window.dispatchEvent(new CustomEvent(ZAP_COMMIT, { detail: n }));
}

function armZapTimer() {
  window.clearTimeout(zapTimer);
  zapTimer = window.setTimeout(commitZap, ZAP_TIMEOUT_MS);
}

export function useKeyboard() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useApp.getState();
      const target = e.target as HTMLElement | null;
      const typing = !!target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.tagName === "SELECT" || target.isContentEditable);

      if (e.key === "Escape") {
        if (typing) {
          (target as HTMLElement).blur();
          if ((target as HTMLInputElement).id === "channel-search") s.setSearch("");
          return;
        }
        if (s.ui.zapDigits) {
          window.clearTimeout(zapTimer);
          return s.setUi({ zapDigits: "" });
        }
        if (s.ui.recordDialog) return s.setUi({ recordDialog: null });
        if (s.rail.kind === "library") return s.selectRail({ kind: "all" });
        if (s.mini) return void s.setMini(false);
        if (s.ui.resumePrompt) return s.setUi({ resumePrompt: null });
        if (s.ui.unlockOpen) return s.setUi({ unlockOpen: false });
        if (s.ui.epgEditChannel) return s.setUi({ epgEditChannel: null });
        if (s.ui.seriesOpen != null) return s.setUi({ seriesOpen: null });
        if (s.ui.fullscreen) return s.setUi({ fullscreen: false });
        if (s.ui.settingsOpen || s.ui.addPlaylistOpen || s.ui.diagnosticsOpen) {
          return s.setUi({ settingsOpen: false, addPlaylistOpen: false, diagnosticsOpen: false });
        }
        return;
      }
      if (typing) {
        if ((target as HTMLInputElement).id === "channel-search") {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            window.dispatchEvent(new CustomEvent(LIST_MOVE, { detail: 1 }));
          } else if (e.key === "ArrowUp") {
            e.preventDefault();
            window.dispatchEvent(new CustomEvent(LIST_MOVE, { detail: -1 }));
          } else if (e.key === "Enter") {
            e.preventDefault();
            window.dispatchEvent(new CustomEvent(LIST_ENTER));
          }
        }
        return;
      }
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      // Modal open: only Esc (handled above) and Enter for the resume prompt.
      if (s.ui.resumePrompt || s.ui.unlockOpen || s.ui.epgEditChannel || s.ui.seriesOpen != null || s.ui.recordDialog) return;

      // Channel-number zap (Live tab): digits build a number, Enter or a pause commits it.
      if (s.tab === "live" && /^[0-9]$/.test(e.key)) {
        e.preventDefault();
        s.setUi({ zapDigits: (s.ui.zapDigits + e.key).slice(0, 4) });
        armZapTimer();
        return;
      }
      if (s.ui.zapDigits) {
        if (e.key === "Enter") {
          e.preventDefault();
          return commitZap();
        }
        if (e.key === "Backspace") {
          e.preventDefault();
          const digits = s.ui.zapDigits.slice(0, -1);
          s.setUi({ zapDigits: digits });
          window.clearTimeout(zapTimer);
          if (digits) armZapTimer();
          return;
        }
      }

      // Letters are matched case-insensitively so Shift/CapsLock combos still work.
      const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
      switch (key) {
        case "/":
          e.preventDefault();
          document.getElementById("channel-search")?.focus();
          break;
        case "j":
        case "ArrowDown":
          e.preventDefault();
          window.dispatchEvent(new CustomEvent(LIST_MOVE, { detail: 1 }));
          break;
        case "k":
        case "ArrowUp":
          e.preventDefault();
          window.dispatchEvent(new CustomEvent(LIST_MOVE, { detail: -1 }));
          break;
        case "PageDown":
          e.preventDefault();
          window.dispatchEvent(new CustomEvent(LIST_MOVE, { detail: 12 }));
          break;
        case "PageUp":
          e.preventDefault();
          window.dispatchEvent(new CustomEvent(LIST_MOVE, { detail: -12 }));
          break;
        case "ArrowLeft":
          e.preventDefault();
          if (s.playback?.is_vod) void s.seekBy(-10);
          else window.dispatchEvent(new CustomEvent(EPG_SHIFT, { detail: -2 }));
          break;
        case "ArrowRight":
          e.preventDefault();
          if (s.playback?.is_vod) void s.seekBy(10);
          else window.dispatchEvent(new CustomEvent(EPG_SHIFT, { detail: 2 }));
          break;
        case "Enter":
          e.preventDefault();
          window.dispatchEvent(new CustomEvent(LIST_ENTER));
          break;
        case "f":
          e.preventDefault();
          s.setUi({ fullscreen: !s.ui.fullscreen });
          break;
        case "m":
          e.preventDefault();
          void s.toggleMute();
          break;
        case " ":
          e.preventDefault();
          void s.togglePause();
          break;
        case "p":
          e.preventDefault();
          void s.toggleProfile();
          break;
        case "r":
          if (s.currentChannel) {
            e.preventDefault();
            window.dispatchEvent(new CustomEvent("diptv:record", { detail: s.currentChannel }));
          }
          break;
        case "i":
          e.preventDefault();
          void s.setMini(!s.mini);
          break;
        case "1":
          s.setTab("live");
          break;
        case "2":
          if (!s.config?.hide_vod_tabs) s.setTab("movies");
          break;
        case "3":
          if (!s.config?.hide_vod_tabs) s.setTab("series");
          break;
        case "s":
          if (e.shiftKey) {
            e.preventDefault();
            s.setUi({ settingsOpen: !s.ui.settingsOpen });
          }
          break;
        case "d":
          if (e.shiftKey) {
            e.preventDefault();
            s.setUi({ diagnosticsOpen: !s.ui.diagnosticsOpen });
          }
          break;
        default:
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
