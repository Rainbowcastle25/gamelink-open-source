import { ChevronDown, CircleAlert, CircleCheck, CircleHelp, FileText, LoaderCircle, Pencil, Play, RefreshCw, Trash2, Unplug } from "lucide-react";
import { useId, useState, type ReactNode } from "react";
import { cn, formatDate, shortMiddle } from "../lib/format";
import { protoLabel } from "../lib/games";
import { hasPanel, serverStatus, type StatusInfo } from "../lib/status";
import type { BusyKind } from "../lib/store";
import type { ServerView } from "../lib/types";
import { Button } from "./Button";
import { ConnectionPanel } from "./ConnectionPanel";
import { GameThumb } from "./GameThumb";

export interface ManageActions {
  onEdit: () => void;
  onRemove: () => void;
  onDiagnostics: () => void;
}

interface Props {
  server: ServerView;
  busy?: BusyKind;
  onJoin: () => void;
  /** Join, then start the game once connected. Shown when the game can be launched. */
  onPlay?: () => void;
  onDisconnect: () => void;
  onCheck: () => void;
  onLaunch: () => void;
  onOpenRequests: () => void;
  /** When given, the card gets a details area with Check / Diagnostics / Edit / Remove. */
  manage?: ManageActions;
}

function StatusIcon({ tone }: { tone: StatusInfo["tone"] }) {
  switch (tone) {
    case "busy":
      return <LoaderCircle className="size-3.5 animate-spin-slow text-accent" aria-hidden />;
    case "ok":
      return <CircleCheck className="size-3.5 text-ok" aria-hidden />;
    case "warn":
      return <CircleAlert className="size-3.5 text-warn" aria-hidden />;
    case "danger":
      return <CircleAlert className="size-3.5 text-danger" aria-hidden />;
    default:
      return <span aria-hidden className="mx-[3px] size-2 rounded-full bg-faint" />;
  }
}

const toneText: Record<StatusInfo["tone"], string> = {
  neutral: "text-muted",
  ok: "text-ok",
  warn: "text-warn",
  danger: "text-danger",
  busy: "text-accent",
};

function Detail({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-xs font-medium tracking-wide text-faint uppercase">{label}</dt>
      <dd className="mt-0.5 truncate text-[13px] text-fg/90">{children}</dd>
    </div>
  );
}

export function ServerCard({ server, busy, onJoin, onPlay, onDisconnect, onCheck, onLaunch, onOpenRequests, manage }: Props) {
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const status = serverStatus(server, busy);
  const c = server.connection;
  const showPanel = hasPanel(server, busy);
  const connecting = c.state === "connecting" || (busy === "joining" && c.state === "disconnected");

  let action;
  if (connecting) {
    action = (
      <Button variant="primary" loading disabled className="min-w-[116px]">
        Connecting...
      </Button>
    );
  } else if (c.state === "connected" || c.state === "reconnecting") {
    action = (
      <Button variant="secondary" onClick={onDisconnect} icon={<Unplug className="size-4" aria-hidden />}>
        Disconnect
      </Button>
    );
  } else if (c.state === "unreachable" || c.state === "error") {
    action = (
      <Button variant="secondary" onClick={onDisconnect}>
        Dismiss
      </Button>
    );
  } else if (onPlay && server.hasExecutable && !server.demo) {
    // One click: connect, then start the game pointed at the local address.
    action = (
      <>
        <Button variant="ghost" size="sm" onClick={onJoin} title="Connect without starting the game">
          Join only
        </Button>
        <Button
          variant="primary"
          onClick={onPlay}
          title="Connect, then start the game and join the server"
          icon={<Play className="size-4" aria-hidden />}
          className="min-w-[88px]"
        >
          Play
        </Button>
      </>
    );
  } else {
    action = (
      <Button
        variant="primary"
        onClick={onJoin}
        title={server.demo ? "Simulated: demo servers do not connect to anything real" : undefined}
        icon={<Play className="size-4" aria-hidden />}
        className="min-w-[88px]"
      >
        Join
      </Button>
    );
  }

  const relayText = (server.relayHosts ?? []).join(", ") || "Unknown";

  return (
    <article
      aria-label={server.name}
      className={cn(
        "min-w-0 rounded-xl border bg-surface shadow-card transition-colors duration-150",
        c.state === "connected" ? "border-teal/30" : "border-line hover:border-line-strong",
      )}
    >
      <div className="flex items-center gap-4 p-4">
        <GameThumb game={server.game} />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-center gap-2">
            <h3 className="truncate text-[15px] font-semibold">{server.name}</h3>
            {server.demo && <span className="shrink-0 rounded-md border border-warn/30 bg-warn/10 px-1.5 py-px text-[11px] font-semibold text-warn">Demo</span>}
          </div>
          {server.description && <p className="mt-0.5 truncate text-[13px] text-muted">{server.description}</p>}
          <div className="mt-1.5 flex min-w-0 items-center gap-2 text-[13px]">
            <span className={cn("inline-flex min-w-0 items-center gap-1.5 font-medium", toneText[status.tone])}>
              <StatusIcon tone={status.tone} />
              <span className="truncate">{status.label}</span>
            </span>
            {status.detail && <span className="shrink-0 text-muted">{status.detail}</span>}
            {status.help && (
              <span title={status.help} aria-label={status.help} role="img" className="shrink-0 cursor-help text-faint hover:text-muted">
                <CircleHelp className="size-3.5" aria-hidden />
              </span>
            )}
            {status.canCheck && !server.demo && (
              <button type="button" onClick={onCheck} className="shrink-0 rounded px-1 text-[13px] text-accent transition-colors duration-150 hover:text-fg hover:underline">
                Check
              </button>
            )}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          {action}
          {manage && (
            <button
              type="button"
              aria-expanded={open}
              aria-controls={detailsId}
              aria-label={open ? `Hide details for ${server.name}` : `Show details for ${server.name}`}
              onClick={() => setOpen((o) => !o)}
              className="grid size-9 place-items-center rounded-lg text-muted transition-colors duration-150 hover:bg-white/6 hover:text-fg"
            >
              <ChevronDown className={cn("size-4 transition-transform duration-200", open && "rotate-180")} aria-hidden />
            </button>
          )}
        </div>
      </div>

      {showPanel && (
        <div className="border-t border-line px-4 py-4">
          <ConnectionPanel
            server={server}
            joining={busy === "joining"}
            onRetry={onJoin}
            onLaunch={onLaunch}
            onOpenRequests={onOpenRequests}
          />
        </div>
      )}

      {manage && open && (
        <div id={detailsId} className="border-t border-line px-4 py-4">
          <dl className="grid grid-cols-2 gap-x-6 gap-y-3.5 min-[1100px]:grid-cols-3">
            <Detail label="Game">{server.gameName || "Other game"}</Detail>
            <Detail label="Protocol and port">
              {protoLabel(server.proto || "tcp")} · tunnel port {server.port}
            </Detail>
            <Detail label="Local port">{server.localPort ? server.localPort : `Automatic (starts at ${server.defaultPort || server.port})`}</Detail>
            <Detail label="Relay">
              <span title={relayText}>{relayText}</span> <span className="text-muted">({server.defaultRelay ? "public relay" : "custom relay"})</span>
            </Detail>
            <Detail label="Server key">
              <code className="font-mono text-xs" title="Fingerprint of the server's public key">
                {server.serverKey ? shortMiddle(server.serverKey, 16, 6) : "Unknown"}
              </code>
            </Detail>
            <Detail label="Added">{formatDate(server.addedAt)}</Detail>
          </dl>
          {!server.defaultRelay && (server.relayHosts?.length ?? 0) > 0 && (
            <p className="mt-3 text-[13px] text-warn">This server uses a custom relay, not the public Tailcat relays.</p>
          )}
          {server.demo ? (
            <p className="mt-4 text-[13px] text-muted">This is a sample server. Turn off Demo Mode in Settings to hide it.</p>
          ) : (
            <div className="mt-4 flex flex-wrap gap-2">
              <Button size="sm" onClick={onCheck} icon={<RefreshCw className="size-3.5" aria-hidden />}>
                Check
              </Button>
              <Button size="sm" onClick={manage.onDiagnostics} icon={<FileText className="size-3.5" aria-hidden />}>
                Diagnostics
              </Button>
              <Button size="sm" onClick={manage.onEdit} icon={<Pencil className="size-3.5" aria-hidden />}>
                Edit
              </Button>
              <Button size="sm" variant="danger" onClick={manage.onRemove} icon={<Trash2 className="size-3.5" aria-hidden />}>
                Remove
              </Button>
            </div>
          )}
        </div>
      )}
    </article>
  );
}
