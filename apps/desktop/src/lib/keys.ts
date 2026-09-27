/**
 * Global keyboard shortcuts (CLAUDE.md §8):
 *   /  search     j/k or ↑/↓ channels     Enter play     f fullscreen
 *   m  mute       space pause             p profile      Esc close/exit
 * List navigation is delegated to the mounted list via DOM events so the list owns its data.
 */
import { useEffect } from "react";
import { useApp } from "./store";

export const LIST_MOVE = "diptv:list-move";
export const LIST_ENTER = "diptv:list-enter";

export function useKeyboard() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useApp.getState();
      const target = e.target as HTMLElement | null;
      const typing = !!target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable);

      if (e.key === "Escape") {
        if (typing) {
          (target as HTMLElement).blur();
          if ((target as HTMLInputElement).id === "channel-search") s.setSearch("");
          return;
        }
        if (s.ui.fullscreen) return s.setUi({ fullscreen: false });
        if (s.ui.settingsOpen || s.ui.addPlaylistOpen || s.ui.diagnosticsOpen) {
          return s.setUi({ settingsOpen: false, addPlaylistOpen: false, diagnosticsOpen: false });
        }
        return;
      }
      if (typing) {
        // Let ↑/↓/Enter work from the search box so results are navigable without leaving it.
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
