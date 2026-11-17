import type { ReactNode } from "react";

interface Props {
  title: string;
  subtitle?: string;
  /** Right-aligned content (status, primary action). */
  children?: ReactNode;
}

export function Header({ title, subtitle, children }: Props) {
  return (
    <header className="mb-7 flex items-start justify-between gap-6">
      <div className="min-w-0">
        <h1 className="text-[26px] leading-tight font-semibold tracking-tight">{title}</h1>
        {subtitle && <p className="mt-1 text-sm text-muted">{subtitle}</p>}
      </div>
      {children && <div className="flex shrink-0 items-center gap-3 pt-1">{children}</div>}
    </header>
  );
}
