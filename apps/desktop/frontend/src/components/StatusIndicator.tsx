import { cn } from "../lib/format";

const MAP: Record<string, { label: string; dot: string; text: string; pulse?: boolean }> = {
  idle: { label: "Ready", dot: "bg-faint", text: "text-muted" },
  connecting: { label: "Connecting", dot: "bg-accent", text: "text-accent", pulse: true },
  connected: { label: "Connected", dot: "bg-ok", text: "text-ok" },
  problem: { label: "Problem", dot: "bg-warn", text: "text-warn" },
};

/** Overall status. Only "connected" is green, and only when the backend verified a tunnel. */
export function StatusIndicator({ status }: { status: string }) {
  const s = MAP[status] ?? MAP.idle;
  return (
    <div role="status" className={cn("inline-flex items-center gap-2 rounded-full border border-line bg-white/3 px-3 py-1.5 text-[13px] font-medium", s.text)}>
      <span aria-hidden className={cn("size-2 rounded-full", s.dot, s.pulse && "animate-pulse-soft")} />
      <span>{s.label}</span>
    </div>
  );
}
