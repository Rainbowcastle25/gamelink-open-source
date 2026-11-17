import { CircleAlert, RefreshCw } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "../lib/format";
import { Button } from "./Button";

interface Props {
  title: string;
  message?: ReactNode;
  onRetry?: () => void;
  retryLabel?: string;
  /** Extra actions beside Retry. */
  children?: ReactNode;
  /** "block" fills a screen area; "inline" sits inside a card. */
  variant?: "block" | "inline";
  tone?: "danger" | "warn";
}

export function ErrorState({ title, message, onRetry, retryLabel = "Retry", children, variant = "block", tone = "danger" }: Props) {
  return (
    <div
      role="alert"
      className={cn(
        "flex gap-3.5 rounded-xl border",
        variant === "block" ? "flex-col items-center px-8 py-12 text-center" : "items-start p-4 text-left",
        tone === "danger" ? "border-danger/25 bg-danger/6" : "border-warn/25 bg-warn/6",
      )}
    >
      <CircleAlert className={cn("shrink-0", variant === "block" ? "size-8" : "mt-0.5 size-5", tone === "danger" ? "text-danger" : "text-warn")} aria-hidden />
      <div className="min-w-0">
        <h3 className="text-[15px] font-semibold">{title}</h3>
        {message && <p className="mt-1 max-w-xl text-[13px] leading-relaxed text-muted">{message}</p>}
        {(onRetry || children) && (
          <div className={cn("mt-3.5 flex flex-wrap gap-2", variant === "block" && "justify-center")}>
            {onRetry && (
              <Button size="sm" variant="secondary" onClick={onRetry} icon={<RefreshCw className="size-3.5" aria-hidden />}>
                {retryLabel}
              </Button>
            )}
            {children}
          </div>
        )}
      </div>
    </div>
  );
}
