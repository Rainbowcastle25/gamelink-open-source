import { CircleAlert, Info, Plug, ShieldCheck, TriangleAlert, Unplug, type LucideIcon } from "lucide-react";
import { relativeTime } from "../lib/format";
import { useAppState, useStore } from "../lib/store";
import { useNow } from "../lib/useNow";

const KIND: Record<string, { icon: LucideIcon; cls: string; label: string }> = {
  info: { icon: Info, cls: "text-faint", label: "Info" },
  connect: { icon: Plug, cls: "text-ok", label: "Connected" },
  disconnect: { icon: Unplug, cls: "text-muted", label: "Disconnected" },
  warning: { icon: TriangleAlert, cls: "text-warn", label: "Warning" },
  error: { icon: CircleAlert, cls: "text-danger", label: "Error" },
  security: { icon: ShieldCheck, cls: "text-accent", label: "Security" },
};

/** Real events from the backend's activity log, newest first. */
export function ActivityPanel() {
  const { activity } = useAppState();
  const store = useStore();
  const now = useNow();
  const items = activity.slice(0, 8);

  return (
    <section aria-labelledby="recent-activity" className="rounded-xl border border-line bg-surface p-5 shadow-card">
      <div className="flex items-center justify-between">
        <h2 id="recent-activity" className="text-sm font-semibold">
          Recent Activity
        </h2>
        {items.length > 0 && (
          <button type="button" onClick={() => void store.clearActivity()} className="rounded px-1 text-xs text-muted transition-colors duration-150 hover:text-fg">
            Clear
          </button>
        )}
      </div>
      {items.length === 0 ? (
        <p className="mt-4 text-[13px] text-muted">Nothing yet</p>
      ) : (
        <ul className="mt-4 space-y-3.5">
          {items.map((e, i) => {
            const k = KIND[String(e.kind)] ?? KIND.info;
            const Icon = k.icon;
            return (
              <li key={`${e.time}-${i}`} className="flex gap-3">
                <Icon className={`mt-0.5 size-4 shrink-0 ${k.cls}`} aria-label={k.label} role="img" />
                <div className="min-w-0">
                  <p className="text-[13px] leading-snug wrap-break-word text-fg/90">{e.message}</p>
                  <p className="mt-0.5 text-xs text-faint">{relativeTime(e.time, now)}</p>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
