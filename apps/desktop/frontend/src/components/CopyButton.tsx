import { Check, Copy } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { errorMessage } from "../lib/format";
import { useStore } from "../lib/store";
import { toast } from "../lib/toast";
import { Button } from "./Button";

interface Props {
  text: string;
  label: string;
  variant?: "primary" | "secondary" | "ghost";
  size?: "sm" | "md";
  className?: string;
}

/** Copies `text` (browser clipboard first, Go clipboard as fallback) and confirms briefly. */
export function CopyButton({ text, label, variant = "secondary", size = "md", className }: Props) {
  const store = useStore();
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const onClick = async () => {
    try {
      let done = false;
      try {
        await navigator.clipboard.writeText(text);
        done = true;
      } catch {
        /* fall back to the native clipboard below */
      }
      if (!done) await store.backend.copyText(text);
      setCopied(true);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1800);
    } catch (e) {
      toast.error(`Could not copy: ${errorMessage(e)}`);
    }
  };

  return (
    <Button variant={variant} size={size} className={className} onClick={() => void onClick()} icon={copied ? <Check className="size-4 text-ok" aria-hidden /> : <Copy className="size-4" aria-hidden />}>
      <span aria-live="polite">{copied ? "Copied" : label}</span>
    </Button>
  );
}
