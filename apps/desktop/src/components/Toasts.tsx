import { useApp } from "../lib/store";
import Icon from "../components/Icon";

export default function Toasts() {
  const toasts = useApp((s) => s.toasts);
  const dismiss = useApp((s) => s.dismissToast);
  if (toasts.length === 0) return null;
  return (
    <div className="fixed bottom-3 right-3 flex flex-col gap-2 z-40">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.level}`}>
          <div className="flex items-start justify-between gap-2">
            <div className="font-semibold text-[12.5px]">{t.title}</div>
            <button className="btn ghost" style={{ padding: "0 4px" }} onClick={() => dismiss(t.id)}>
              <Icon name="close" size={14} />
            </button>
          </div>
          {t.body && (
            <div className="mt-1 whitespace-pre-wrap break-words" style={{ color: "var(--text-dim)", fontSize: 12 }}>
              {t.body}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
