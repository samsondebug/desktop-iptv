/**
 * Self-update: `latest.json` on GitHub Releases, signed with the project's minisign key (the
 * public half is in tauri.conf.json → plugins.updater.pubkey). A signature mismatch or a missing
 * manifest is a silent no-op for the background check and a plain message for a manual one.
 *
 * Windows: the NSIS installer runs in passive mode and the app exits by itself. macOS/Linux: the
 * bundle is replaced in place and we relaunch.
 */
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { useApp } from "./store";

export type UpdateStatus =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "none"; checkedAt: number }
  | { phase: "available"; version: string; notes?: string; date?: string }
  | { phase: "downloading"; version: string; done: number; total: number | null }
  | { phase: "installing"; version: string }
  | { phase: "error"; message: string };

let pending: Update | null = null;
let inFlight: Promise<void> | null = null;

function setStatus(update: UpdateStatus) {
  useApp.setState({ update });
}

/** Look for a newer release. `interactive` = the user pressed the button, so errors are shown. */
export async function checkForUpdates(interactive: boolean): Promise<void> {
  if (inFlight) return inFlight;
  inFlight = (async () => {
    setStatus({ phase: "checking" });
    try {
      const u = await check({ timeout: 15_000 });
      if (!u) {
        pending = null;
        setStatus({ phase: "none", checkedAt: Date.now() });
        return;
      }
      pending = u;
      setStatus({ phase: "available", version: u.version, notes: u.body ?? undefined, date: u.date ?? undefined });
      useApp.getState().pushToast({
        level: "info",
        sticky: true,
        title: `SKTV ${u.version} is available`,
        body: u.body ? u.body.split("\n").slice(0, 3).join("\n") : "Installs in the background, then restarts.",
        action: { label: "Install & restart", onClick: () => void installUpdate() },
      });
    } catch (e) {
      // No manifest yet (fresh repo), offline, or the signature did not verify. Background checks
      // stay quiet; a manual check says what happened.
      const message = String(e);
      pending = null;
      if (interactive) setStatus({ phase: "error", message });
      else setStatus({ phase: "idle" });
    } finally {
      inFlight = null;
    }
  })();
  return inFlight;
}

export async function installUpdate(): Promise<void> {
  const u = pending;
  if (!u) return;
  let total: number | null = null;
  let done = 0;
  setStatus({ phase: "downloading", version: u.version, done, total });
  try {
    await u.downloadAndInstall((ev) => {
      if (ev.event === "Started") {
        total = ev.data.contentLength ?? null;
        setStatus({ phase: "downloading", version: u.version, done, total });
      } else if (ev.event === "Progress") {
        done += ev.data.chunkLength;
        setStatus({ phase: "downloading", version: u.version, done, total });
      } else if (ev.event === "Finished") {
        setStatus({ phase: "installing", version: u.version });
      }
    });
    // Windows exits inside downloadAndInstall once the installer is running; elsewhere relaunch.
    await relaunch();
  } catch (e) {
    setStatus({ phase: "error", message: String(e) });
    useApp.getState().pushToast({ level: "error", title: "Update failed", body: String(e) });
  }
}

/** One quiet check shortly after start, then every 6 hours while the app stays open. */
export function scheduleBackgroundChecks(): () => void {
  const first = setTimeout(() => void checkForUpdates(false), 12_000);
  const every = setInterval(() => void checkForUpdates(false), 6 * 60 * 60 * 1000);
  return () => {
    clearTimeout(first);
    clearInterval(every);
  };
}
