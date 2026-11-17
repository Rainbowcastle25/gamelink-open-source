import { LoaderCircle } from "lucide-react";
import type { ButtonHTMLAttributes, ReactNode } from "react";
import { cn } from "../lib/format";

type Variant = "primary" | "secondary" | "ghost" | "danger";
type Size = "sm" | "md";

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  loading?: boolean;
  icon?: ReactNode;
}

const base =
  "inline-flex items-center justify-center gap-2 rounded-lg font-medium whitespace-nowrap select-none transition-[background-color,border-color,color,opacity,box-shadow] duration-150 disabled:opacity-55";

const variants: Record<Variant, string> = {
  primary:
    "bg-linear-to-br from-accent to-teal text-[#04141c] shadow-card hover:brightness-110 active:brightness-95 disabled:hover:brightness-100",
  secondary: "border border-line-strong bg-white/4 text-fg hover:bg-white/8 active:bg-white/10",
  ghost: "text-muted hover:bg-white/6 hover:text-fg",
  danger: "border border-danger/35 bg-danger/10 text-danger hover:bg-danger/18",
};

const sizes: Record<Size, string> = {
  sm: "h-8 px-3 text-[13px]",
  md: "h-9 px-4 text-sm",
};

export function Button({ variant = "secondary", size = "md", loading, icon, className, children, disabled, type = "button", ...rest }: Props) {
  return (
    <button type={type} className={cn(base, variants[variant], sizes[size], className)} disabled={disabled || loading} {...rest}>
      {loading ? <LoaderCircle className="size-4 animate-spin-slow" aria-hidden /> : icon}
      {children}
    </button>
  );
}
