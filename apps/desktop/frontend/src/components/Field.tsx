import { useId, type ReactNode } from "react";

export const inputCls =
  "w-full rounded-lg border border-line-strong bg-black/25 px-3 py-2 text-sm text-fg placeholder:text-faint transition-colors duration-150 hover:border-white/20 focus:border-accent/60 disabled:opacity-55";

interface Props {
  label: string;
  hint?: ReactNode;
  error?: string | null;
  /** Receives the id to put on the control, plus the ids of hint/error text for aria-describedby. */
  children: (ids: { id: string; describedBy?: string }) => ReactNode;
}

export function Field({ label, hint, error, children }: Props) {
  const id = useId();
  const hintId = `${id}-hint`;
  const errId = `${id}-err`;
  const describedBy = [hint ? hintId : "", error ? errId : ""].filter(Boolean).join(" ") || undefined;
  return (
    <div>
      <label htmlFor={id} className="mb-1.5 block text-[13px] font-medium">
        {label}
      </label>
      {children({ id, describedBy })}
      {hint && (
        <p id={hintId} className="mt-1.5 text-xs leading-relaxed text-muted">
          {hint}
        </p>
      )}
      {error && (
        <p id={errId} role="alert" className="mt-1.5 text-xs leading-relaxed text-danger">
          {error}
        </p>
      )}
    </div>
  );
}
