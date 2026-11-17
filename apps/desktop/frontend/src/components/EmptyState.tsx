import type { ReactNode } from "react";

interface Props {
  icon?: ReactNode;
  title: string;
  description?: ReactNode;
  children?: ReactNode;
}

/** Calm "nothing here yet" block with optional call-to-action buttons. */
export function EmptyState({ icon, title, description, children }: Props) {
  return (
    <div className="flex flex-col items-center rounded-xl border border-dashed border-line-strong bg-surface/60 px-8 py-12 text-center">
      {icon && <div className="mb-4 grid size-12 place-items-center rounded-xl bg-white/5 text-accent">{icon}</div>}
      <h3 className="text-base font-semibold">{title}</h3>
      {description && <p className="mt-1.5 max-w-md text-[13px] leading-relaxed text-muted">{description}</p>}
      {children && <div className="mt-5 flex flex-wrap items-center justify-center gap-2.5">{children}</div>}
    </div>
  );
}
