/**
 * Channel-number zap (TiviMate-style): digits typed on the Live tab build a number shown in a
 * big overlay; Enter or a short pause plays that row of the current list (rail + search order —
 * the same numbers the list shows). Mounted independently of the list so it also works in
 * fullscreen, where no list is on screen.
 */
import { useEffect } from "react";
import { ZAP_COMMIT } from "../../lib/keys";
import { useApp } from "../../lib/store";
import { useChannelSource } from "./useChannelSource";

export default function ZapOverlay() {
  const src = useChannelSource();
  const digits = useApp((s) => s.ui.zapDigits);
  const play = useApp((s) => s.play);
  const setUi = useApp((s) => s.setUi);
  const pushToast = useApp((s) => s.pushToast);

  useEffect(() => {
    const onZap = (e: Event) => {
      const n = (e as CustomEvent<number>).detail;
      const idx = n - 1;
      if (idx < 0 || idx >= src.count) {
        pushToast({ level: "warn", title: `No channel ${n}`, body: `This list has ${src.count.toLocaleString()} channels.` });
        return;
      }
      void src.fetchRow(idx).then((ch) => {
        if (!ch) return;
        setUi({ selectedIndex: idx });
        void play(ch);
      });
    };
    window.addEventListener(ZAP_COMMIT, onZap);
    return () => window.removeEventListener(ZAP_COMMIT, onZap);
  }, [src, play, setUi, pushToast]);

  if (!digits) return null;
  const n = parseInt(digits, 10);
  const target = n > 0 && n <= src.count ? src.row(n - 1) : undefined;
  return (
    <div className="fixed z-50 pointer-events-none" style={{ top: 64, right: 24 }}>
      <div className="panel rounded-md px-4 py-2 text-right" style={{ boxShadow: "0 10px 30px rgba(0,0,0,.5)", minWidth: 120 }}>
        <div style={{ fontSize: 34, fontWeight: 700, letterSpacing: "0.08em", fontVariantNumeric: "tabular-nums" }}>
          {digits}
          <span style={{ opacity: 0.35 }}>_</span>
        </div>
        <div className="truncate" style={{ color: "var(--text-dim)", fontSize: 12, maxWidth: 220 }}>
          {n > src.count ? `Only ${src.count.toLocaleString()} channels` : target?.name ?? "…"}
        </div>
      </div>
    </div>
  );
}
