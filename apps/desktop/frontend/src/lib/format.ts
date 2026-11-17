/** Turns anything thrown (Native command rejections, Errors, strings) into a sentence. */
export function errorMessage(e: unknown): string {
  if (e instanceof Error && e.message) return e.message;
  if (typeof e === "string" && e) return e;
  if (e && typeof e === "object" && "message" in e) {
    const m = (e as { message: unknown }).message;
    if (typeof m === "string" && m) return m;
  }
  return "Something went wrong.";
}

/** "2m ago" style relative time; falls back to a date for old events. */
export function relativeTime(iso: string, now: number = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 10) return "Just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 14) return `${d}d ago`;
  return new Date(t).toLocaleDateString();
}

export function formatMs(ms: number): string {
  return `${Math.round(ms)} ms`;
}

export function formatDate(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return iso || "Unknown";
  return new Date(t).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

/** Shortens a long identifier to head...tail for display only. */
export function shortMiddle(s: string, head = 14, tail = 6): string {
  if (s.length <= head + tail + 1) return s;
  return `${s.slice(0, head)}…${s.slice(-tail)}`;
}

/**
 * Parses an optional port field. Returns 0 for "empty / automatic", a number
 * for a valid port, or null if the text is not a valid port.
 */
export function parseOptionalPort(text: string): number | null {
  const t = text.trim();
  if (t === "") return 0;
  if (!/^\d{1,5}$/.test(t)) return null;
  const n = Number(t);
  return n >= 1 && n <= 65535 ? n : null;
}

export function parseRequiredPort(text: string): number | null {
  const n = parseOptionalPort(text);
  return n === null || n === 0 ? null : n;
}

export function cn(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

/** Backend errors are lower-case Go sentences; show them as proper sentences. */
export function sentence(msg: string): string {
  const t = msg.trim();
  if (!t) return t;
  const s = t[0].toUpperCase() + t.slice(1);
  return /[.!?]$/.test(s) ? s : `${s}.`;
}
