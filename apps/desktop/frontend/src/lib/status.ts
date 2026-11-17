import type { BusyKind } from "./store";
import { formatMs } from "./format";
import type { ServerView } from "./types";

export type StatusTone = "neutral" | "ok" | "warn" | "danger" | "busy";

export interface StatusInfo {
  label: string;
  /** Secondary text, e.g. "34 ms". Only present when the backend measured it. */
  detail?: string;
  tone: StatusTone;
  /** Offer a small "Check" action. */
  canCheck?: boolean;
  /** Longer explanation, shown as a tooltip. */
  help?: string;
}

export const NO_RESPONSE_HELP =
  "The server did not answer. The protocol cannot tell an offline server apart from a device that has not been approved for this game. Make sure the server is running and that its owner has approved your public key (see Requests).";

/**
 * The one-line status of a server. A live connection state always wins over the
 * last reachability check; nothing is invented (no player counts, no guessed latency).
 */
export function serverStatus(s: ServerView, busy?: BusyKind): StatusInfo {
  const c = s.connection;
  switch (c.state) {
    case "connecting":
      return { label: "Connecting...", tone: "busy" };
    case "connected":
      if (c.gameReachable === "no") {
        return { label: "Game server offline", tone: "warn", detail: c.latencyMs != null ? formatMs(c.latencyMs) : undefined };
      }
      if (c.gameReachable !== "yes") {
        return { label: "Agent online, game status unknown", tone: "neutral", detail: c.latencyMs != null ? formatMs(c.latencyMs) : undefined };
      }
      return { label: "Connected", tone: "ok", detail: c.latencyMs != null ? formatMs(c.latencyMs) : undefined };
    case "reconnecting":
      return { label: "Reconnecting...", tone: "busy" };
    case "unreachable":
      return { label: "No response", tone: "warn", help: NO_RESPONSE_HELP };
    case "error":
      return { label: "Connection failed", tone: "danger" };
  }
  if (busy === "joining") return { label: "Connecting...", tone: "busy" };
  if (busy === "checking" || s.check.state === "checking") return { label: "Checking...", tone: "busy" };
  if (s.demo) return { label: "Sample server", tone: "neutral", detail: "simulated" };
  switch (s.check.state) {
    case "online":
      return { label: "Online", tone: "ok", detail: s.check.latencyMs != null ? formatMs(s.check.latencyMs) : undefined };
    case "offline":
      return { label: "Game server offline", tone: "warn", canCheck: true };
    case "unknown":
      if (s.check.message === "Agent online, game status unknown") {
        return { label: "Agent online, game status unknown", tone: "neutral", canCheck: true };
      }
      return { label: "Not checked", tone: "neutral", canCheck: true };
    case "unreachable":
      return { label: "No response", tone: "warn", canCheck: true, help: NO_RESPONSE_HELP };
    default:
      return { label: "Not checked", tone: "neutral", canCheck: true };
  }
}

/** Connection states that show the expanded panel under a card. */
export function hasPanel(s: ServerView, busy?: BusyKind): boolean {
  return busy === "joining" || ["connecting", "connected", "reconnecting", "unreachable", "error"].includes(s.connection.state);
}
