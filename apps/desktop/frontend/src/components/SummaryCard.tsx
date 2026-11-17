import type { ReactNode } from "react";

interface Props {
  icon: ReactNode;
  label: string;
  value: ReactNode;
  note: ReactNode;
}

export function SummaryCard({ icon, label, value, note }: Props) {
  return (
    <section aria-label={label} className="min-w-0 rounded-xl border border-line bg-surface p-5 shadow-card">
      <div className="flex items-center gap-2 text-[13px] font-medium text-muted">
        <span className="text-accent/90">{icon}</span>
        {label}
      </div>
      <div className="mt-3 truncate text-[28px] leading-none font-semibold tracking-tight">{value}</div>
      <p className="mt-2.5 text-[13px] leading-snug text-muted">{note}</p>
    </section>
  );
}
