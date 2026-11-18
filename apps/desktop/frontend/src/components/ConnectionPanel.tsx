import { Info, LoaderCircle, Play, Route, TriangleAlert, Unplug } from "lucide-react";
import type { ReactNode } from "react";
import { formatMs } from "../lib/format";
import type { ServerView } from "../lib/types";
import { Button } from "./Button";
import { CopyButton } from "./CopyButton";
import { ErrorState } from "./ErrorState";

interface Props {
  server: ServerView;
  /** Treat as "connecting" even before the backend reports it (optimistic Join). */
  joining?: boolean;
  onDisconnect?: () => void;
  onRetry?: () => void;
  onLaunch?: () => void;
  onOpenRequests?: () => void;
}

function pathText(path: string): string {
  if (path === "direct") return "Direct connection";
  if (path === "relay") return "Relayed";
  return "Path not measured";
}

function Note({ children }: { children: ReactNode }) {
  return (
    <div className="flex items-start gap-2.5 rounded-lg border border-warn/25 bg-warn/8 px-3.5 py-2.5 text-[13px] leading-snug text-[#f6d38a]">
      <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warn" aria-hidden />
      <p>{children}</p>
    </div>
  );
}

/** The live part of a card: address, path, warnings, instructions, or a clear explanation of what went wrong. */
export function ConnectionPanel({ server, joining, onDisconnect, onRetry, onLaunch, onOpenRequests }: Props) {
  const c = server.connection;
  const state = joining && c.state === "disconnected" ? "connecting" : c.state;
  // The synced list entry follows the port the tunnel actually bound.
  const listed = server.minecraftListed;
  const hint = listed
    ? `In Minecraft Multiplayer, select “${server.name} (GameLink)”.`
    : server.launchConnects ? "Launch the game from GameLink to join automatically." : server.hint;
  const address = (
    <div className="flex items-stretch gap-2.5">
      <code className="min-w-0 flex-1 truncate rounded-lg border border-line-strong bg-black/30 px-4 py-2.5 font-mono text-lg tracking-tight select-all" aria-label="Local address">
        {c.localAddr}
      </code>
      <CopyButton text={c.localAddr} label="Copy Address" variant="primary" />
    </div>
  );

  if (state === "connecting") {
    return (
      <div className="flex items-center gap-3 text-[13px] text-muted" role="status">
        <LoaderCircle className="size-4 animate-spin-slow text-accent" aria-hidden />
        GameLink is finding the best path to the server. This can take a few seconds.
      </div>
    );
  }

  if (state === "unreachable") {
    return (
      <ErrorState
        variant="inline"
        tone="warn"
        title="No response"
        message="The server may be offline, or this device has not been approved for this game. Send the owner your public key (Requests tab)."
        onRetry={onRetry}
      >
        {onOpenRequests && (
          <Button size="sm" variant="ghost" onClick={onOpenRequests}>
            Open Requests
          </Button>
        )}
      </ErrorState>
    );
  }

  if (state === "error") {
    return <ErrorState variant="inline" title="Could not connect" message={c.message || "The connection failed."} onRetry={onRetry} />;
  }

  // connected or reconnecting
  return (
    <div className="space-y-4">
      {state === "reconnecting" && (
        <div className="flex items-center gap-3 rounded-lg bg-accent/8 px-3.5 py-2.5 text-[13px]" role="status">
          <LoaderCircle className="size-4 animate-spin-slow text-accent" aria-hidden />
          Reconnecting... The connection was interrupted and GameLink is restoring it.
        </div>
      )}

      {c.portFallback && (
        <Note>
          {server.stablePort ? `Stable port ${server.stablePort}` : "Stable port"} was unavailable. Using temporary port {c.localPort}.
        </Note>
      )}
      {c.localAddr && (listed || (server.launchConnects && !c.portFallback) ? (
        <details>
          <summary className="cursor-pointer text-xs font-medium text-muted">Show local address</summary>
          <div className="mt-2">{address}</div>
        </details>
      ) : (
        <div>
          <p className="mb-1.5 text-xs font-medium tracking-wide text-muted uppercase">Local address</p>
          {address}
        </div>
      ))}

      <div className="flex flex-wrap items-center gap-x-5 gap-y-1 text-[13px] text-muted">
        <span className="inline-flex items-center gap-1.5">
          <Route className="size-3.5" aria-hidden />
          {pathText(c.path)}
        </span>
        {c.latencyMs != null && <span>{formatMs(c.latencyMs)}</span>}
        {c.reconnects > 0 && <span>{c.reconnects === 1 ? "Reconnected once" : `Reconnected ${c.reconnects} times`}</span>}
      </div>

      {(c.simulated || server.demo) && (
        <div className="flex items-start gap-2.5 text-[13px] text-muted">
          <Info className="mt-0.5 size-4 shrink-0 text-accent" aria-hidden />
          This is a simulated connection. Demo servers are samples: no network traffic is sent.
        </div>
      )}

      {c.gameReachable === "no" && <Note>Connected to the server, but the game server is not accepting connections - is it running?</Note>}
      {c.gameReachable !== "yes" && c.gameReachable !== "no" && !c.simulated && !server.demo && <Note>GameLink is connected, but the game server status is unknown. The game endpoint has not responded to a check yet.</Note>}
      {c.oversizeDrops > 0 && (
        <Note>
          {c.oversizeDrops === 1 ? "1 oversized UDP packet was dropped" : `${c.oversizeDrops} oversized UDP packets were dropped`}. GameLink can carry UDP packets up to 1232 bytes; larger
          ones cannot pass through the tunnel.
        </Note>
      )}

      {hint && (
        <div>
          <p className="mb-1 text-xs font-medium tracking-wide text-muted uppercase">How to connect</p>
          <p className="text-[13px] leading-relaxed text-fg/85">{hint}</p>
        </div>
      )}

      {(onLaunch && server.hasExecutable && !server.demo) || onDisconnect ? (
        <div className="flex flex-wrap gap-2.5">
          {onLaunch && server.hasExecutable && !server.demo && (
            <Button variant="secondary" onClick={onLaunch} icon={<Play className="size-4" aria-hidden />}>
              Launch Game
            </Button>
          )}
          {onDisconnect && (
            <Button variant="secondary" onClick={onDisconnect} icon={<Unplug className="size-4" aria-hidden />}>
              Disconnect
            </Button>
          )}
        </div>
      ) : null}
    </div>
  );
}
