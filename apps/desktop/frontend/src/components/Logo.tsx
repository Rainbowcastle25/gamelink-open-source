import { useId } from "react";

/** GameLink mark: two linked rounded arcs in a cyan-to-teal gradient (original artwork). */
export function LogoMark({ className = "size-8" }: { className?: string }) {
  const id = useId();
  return (
    <svg viewBox="0 0 64 64" className={className} role="img" aria-label="GameLink logo">
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#22d3ee" />
          <stop offset="1" stopColor="#0d9488" />
        </linearGradient>
      </defs>
      <rect width="64" height="64" rx="16" fill="#0b1426" stroke="rgba(255,255,255,0.08)" />
      <path
        d="M22 24a10 10 0 0 0 0 16h5M42 24a10 10 0 0 1 0 16h-5M25 32h14"
        fill="none"
        stroke={`url(#${id})`}
        strokeWidth="5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function Logo() {
  return (
    <div className="flex items-center gap-2.5">
      <LogoMark className="size-8" />
      <span className="text-[17px] font-semibold tracking-tight">GameLink</span>
    </div>
  );
}
