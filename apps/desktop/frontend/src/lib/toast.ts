import { useSyncExternalStore } from "react";

export type ToastKind = "success" | "error" | "info";

export interface ToastItem {
  id: number;
  kind: ToastKind;
  message: string;
}

let items: ToastItem[] = [];
let nextId = 1;
const listeners = new Set<() => void>();
const timers = new Map<number, ReturnType<typeof setTimeout>>();

function emit() {
  for (const l of listeners) l();
}

function push(kind: ToastKind, message: string, ms: number) {
  const id = nextId++;
  items = [...items.slice(-3), { id, kind, message }];
  timers.set(
    id,
    setTimeout(() => dismiss(id), ms),
  );
  emit();
}

export function dismiss(id: number) {
  const t = timers.get(id);
  if (t) clearTimeout(t);
  timers.delete(id);
  items = items.filter((i) => i.id !== id);
  emit();
}

/** Clears every toast (used by tests). */
export function resetToasts() {
  for (const t of timers.values()) clearTimeout(t);
  timers.clear();
  items = [];
  emit();
}

export const toast = {
  success: (message: string) => push("success", message, 3500),
  info: (message: string) => push("info", message, 4000),
  error: (message: string) => push("error", message, 7000),
};

function subscribe(l: () => void) {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
}

export function useToasts(): ToastItem[] {
  return useSyncExternalStore(
    subscribe,
    () => items,
    () => items,
  );
}
