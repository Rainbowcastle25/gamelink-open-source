import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { dismiss, useToasts } from "../lib/toast";
import { cn } from "../lib/format";

const icons = {
  success: <CircleCheck className="size-4 shrink-0 text-ok" aria-hidden />,
  error: <CircleAlert className="size-4 shrink-0 text-danger" aria-hidden />,
  info: <Info className="size-4 shrink-0 text-accent" aria-hidden />,
};

/** Lightweight notifications. The region is always mounted so screen readers announce additions. */
export function Toaster() {
  const items = useToasts();
  return (
    <div aria-live="polite" aria-atomic="false" className="pointer-events-none fixed right-5 bottom-5 z-50 flex w-[360px] max-w-[calc(100vw-40px)] flex-col gap-2">
      {items.map((t) => (
        <div
          key={t.id}
          role={t.kind === "error" ? "alert" : "status"}
          className={cn(
            "pointer-events-auto flex items-start gap-3 rounded-xl border bg-surface-2 px-4 py-3 text-[13px] shadow-pop animate-pop-in",
            t.kind === "error" ? "border-danger/40" : "border-line-strong",
          )}
        >
          {icons[t.kind]}
          <p className="min-w-0 flex-1 wrap-break-word">{t.message}</p>
          <button type="button" aria-label="Dismiss notification" onClick={() => dismiss(t.id)} className="-m-1 rounded p-1 text-muted hover:text-fg">
            <X className="size-3.5" aria-hidden />
          </button>
        </div>
      ))}
    </div>
  );
}
