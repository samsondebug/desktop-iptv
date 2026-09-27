import { useApp } from "../lib/store";

/** First-run legal block (CLAUDE.md non-negotiable 7). Also shown in Settings. */
export default function LegalGate() {
  const boot = useApp((s) => s.boot)!;
  const acceptLegal = useApp((s) => s.acceptLegal);
  return (
    <div className="opaque h-full w-full flex items-center justify-center">
      <div className="modal" style={{ width: "min(560px, 92vw)" }}>
        <div className="text-[18px] font-semibold mb-2">Before you start</div>
        <p style={{ color: "var(--text)", lineHeight: 1.55, fontSize: 14 }}>{boot.legal_block}</p>
        <ul className="mt-3 mb-4 pl-5 list-disc" style={{ color: "var(--text-dim)", lineHeight: 1.6 }}>
          <li>You add your own M3U, Xtream, or Stalker source.</li>
          <li>Nothing is collected or sent anywhere. Analytics are off.</li>
          <li>Buffering and delay usually come from the provider or the network — Diagnostics shows which.</li>
        </ul>
        <div className="flex justify-end gap-2">
          <button className="btn primary" onClick={() => void acceptLegal()}>
            I understand
          </button>
        </div>
      </div>
    </div>
  );
}
