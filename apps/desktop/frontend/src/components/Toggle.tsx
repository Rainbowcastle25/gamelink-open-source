import { cn } from "../lib/format";

interface Props {
  checked: boolean;
  onChange: (next: boolean) => void;
  /** Accessible name (also what tests and screen readers use). */
  label: string;
  disabled?: boolean;
}

export function Toggle({ checked, onChange, label, disabled }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative h-6 w-11 shrink-0 rounded-full border transition-colors duration-200 disabled:opacity-50",
        checked ? "border-teal/60 bg-teal/80" : "border-line-strong bg-white/8",
      )}
    >
      <span
        aria-hidden
        className={cn(
          "absolute top-1/2 left-0.5 size-[18px] -translate-y-1/2 rounded-full bg-white shadow-card transition-transform duration-200",
          checked && "translate-x-5",
        )}
      />
    </button>
  );
}
