import { cn } from "../lib/format";

export function Skeleton({ className }: { className?: string }) {
  return <div aria-hidden className={cn("animate-pulse-soft rounded-lg bg-white/6", className)} />;
}

/** Placeholder rows shown during the initial load. */
export function ServerListSkeleton({ rows = 3 }: { rows?: number }) {
  return (
    <div role="status" aria-label="Loading servers" className="flex flex-col gap-3">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-4 rounded-xl border border-line bg-surface p-4">
          <Skeleton className="size-12 rounded-xl" />
          <div className="flex-1 space-y-2">
            <Skeleton className="h-4 w-1/3" />
            <Skeleton className="h-3 w-1/2" />
          </div>
          <Skeleton className="h-9 w-20" />
        </div>
      ))}
    </div>
  );
}

export function SummarySkeleton() {
  return (
    <div role="status" aria-label="Loading summary" className="grid grid-cols-2 gap-4">
      {[0, 1].map((i) => (
        <div key={i} className="rounded-xl border border-line bg-surface p-5">
          <Skeleton className="h-3 w-16" />
          <Skeleton className="mt-4 h-8 w-12" />
          <Skeleton className="mt-3 h-3 w-24" />
        </div>
      ))}
    </div>
  );
}
