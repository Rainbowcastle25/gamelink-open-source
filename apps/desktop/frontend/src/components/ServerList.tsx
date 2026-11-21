import { FileInput, Plus, Server as ServerIcon } from "lucide-react";
import { useNav } from "../lib/nav";
import { useAppState, useStore } from "../lib/store";
import { useHostedGames, useLinkedGames } from "../lib/useHostedGames";
import type { ServerView } from "../lib/types";
import { Button } from "./Button";
import { EmptyState } from "./EmptyState";
import { ErrorState } from "./ErrorState";
import { GameThumb } from "./GameThumb";
import { HostedCardDetails } from "./HostedCardDetails";
import { ServerCard } from "./ServerCard";
import { ServerListSkeleton } from "./Skeleton";

export interface ManageHandlers {
  onEdit: (s: ServerView) => void;
  onRemove: (s: ServerView) => void;
  onDiagnostics: (s: ServerView) => void;
}

export function useServerList() {
  const { servers } = useAppState();
  const store = useStore();
  const local = useHostedGames(store.backend);
  const linked = useLinkedGames(store.backend);
  const hosted = [
    ...(local.games ?? []).map((game) => ({
      key: `local-${game.id}`, name: game.name, game: game.game, serverFingerprint: game.serverFingerprint,
      tunnel: undefined,
      legacyAgent: false,
      detail: `Hosted on this PC · Port ${game.port} · ${game.enabled ? game.state === "serving" ? "Tunnel active" : game.detail : "Sharing paused"}`,
      copy: game.enabled || game.lan ? () => store.backend.copyHostedInvitation(game.id) : undefined,
    })),
    ...(linked.games ?? []).flatMap((game) => {
      const { serverFingerprint } = game;
      if (!serverFingerprint && !game.legacyAgent) return [];
      return [{
        key: `linked-${game.id}`, name: game.name, game: game.type, serverFingerprint,
        tunnel: game.proto && game.port !== undefined ? { proto: game.proto, port: game.port } : undefined,
        legacyAgent: game.legacyAgent,
        detail: `Hosted on your linked node${game.port ? ` · Port ${game.port}` : ""} · ${game.disabled ? "Disabled" : game.detail || game.state}`,
        copy: serverFingerprint && !game.legacyAgent && game.canCopyInvitation ? () => store.backend.copyLinkedInvitation(game.id, serverFingerprint) : undefined,
      }];
    }),
  ];
  const matches = (server: ServerView, game: (typeof hosted)[number]) =>
    !server.demo && !!game.serverFingerprint && server.serverFingerprint === game.serverFingerprint;
  const hasCurrentSettings = (server: ServerView, game: (typeof hosted)[number]) =>
    server.game === game.game && (!game.tunnel || (server.proto === game.tunnel.proto && server.port === game.tunnel.port));
  const savedCards = new Map(hosted.map((game) => {
    const candidates = servers.filter((server) => matches(server, game));
    const saved = candidates.find((server) => hasCurrentSettings(server, game)) ?? candidates[0];
    return [game.key, saved?.id] as const;
  }));
  const hostedOnly = hosted.filter((game) => !savedCards.get(game.key));

  const count = servers.filter((server) => !server.demo).length + hostedOnly.length;
  return { local, linked, hosted, hostedOnly, savedCards, matches, hasCurrentSettings, count };
}

/** The vertical list of server cards, wired to the store. Also owns the loading / error / empty states. */
export function ServerList({ list, manage }: {
  list: ReturnType<typeof useServerList>;
  manage?: ManageHandlers;
}) {
  const { ready, loadError, servers, busy, gameIcons } = useAppState();
  const store = useStore();
  const { go } = useNav();
  const { local, linked, hosted, hostedOnly, savedCards, matches, hasCurrentSettings } = list;

  if (!ready) return <ServerListSkeleton />;

  const savedError = loadError && servers.length === 0 && (
    <ErrorState
      variant={hostedOnly.length ? "inline" : "block"}
      title="Could not load your servers"
      message={`${loadError} If this keeps happening, restart GameLink.`}
      onRetry={() => void store.retryLoad()}
    />
  );

  const localError = local.error && (
    <ErrorState
      variant={servers.length || hostedOnly.length ? "inline" : "block"}
      title="Could not load games hosted on this PC"
      message={local.error}
      onRetry={() => void local.refresh()}
      retryLabel="Retry hosted games"
    />
  );

  const linkedError = linked.error && (
    <ErrorState
      variant={servers.length || hostedOnly.length ? "inline" : "block"}
      title="Could not load games on your linked node"
      message={linked.error}
      onRetry={() => void linked.refresh()}
      retryLabel="Retry linked node"
    />
  );

  const linkedRefreshing = linked.refreshing && (
    <p role="status" className="text-sm text-muted">Refreshing…</p>
  );

  if (servers.length === 0 && hostedOnly.length === 0) {
    if (savedError || localError || linkedError || linkedRefreshing) return <div className="space-y-3">{savedError}{localError}{linkedError}{linkedRefreshing}</div>;
    if ((!local.games && !local.error) || (!linked.games && !linked.error)) return <ServerListSkeleton />;
    return (
      <EmptyState
        icon={<ServerIcon className="size-6" aria-hidden />}
        title="No servers yet"
        description="Ask a friend who hosts a game server to send you an invitation. Paste it here and you can join without changing any router settings."
      >
        <Button variant="primary" onClick={() => go("servers", { openImport: true })} icon={<FileInput className="size-4" aria-hidden />}>
          Import invitation
        </Button>
        <Button onClick={() => go("servers", { openAdd: true })} icon={<Plus className="size-4" aria-hidden />}>
          Add server manually
        </Button>
      </EmptyState>
    );
  }

  return (
    <ul className="flex flex-col gap-3">
      {savedError && <li>{savedError}</li>}
      {localError && <li>{localError}</li>}
      {linkedError && <li>{linkedError}</li>}
      {linkedRefreshing && <li>{linkedRefreshing}</li>}
      {hostedOnly.map((game) => (
        <li key={game.key} className="min-w-0">
          <article aria-label={game.name} className="flex min-w-0 items-start gap-4 rounded-xl border border-line bg-surface p-4 shadow-card">
            <GameThumb game={game.game} src={gameIcons[game.game]} />
            <div className="min-w-0 flex-1">
              <h3 className="text-[15px] font-semibold [overflow-wrap:anywhere]">{game.name}</h3>
              <HostedCardDetails name={game.name} detail={game.detail} copy={game.copy} legacyAgent={game.legacyAgent} />
            </div>
          </article>
        </li>
      ))}
      {servers.map((s) => {
        const game = hosted.find((entry) => savedCards.get(entry.key) === s.id);
        const stale = hosted.some((entry) => entry.tunnel && matches(s, entry) && !hasCurrentSettings(s, entry));
        return (
          <li key={s.id} className="min-w-0">
            <ServerCard
              server={s}
              gameIcon={gameIcons[s.game]}
              busy={busy[s.id]}
              hosted={(game || stale) && <>
                {game && <HostedCardDetails name={s.name} detail={game.detail} copy={game.copy} legacyAgent={game.legacyAgent} />}
                {stale && <p className="text-[13px] text-warn">Saved connection settings are out of date. Add this game again from Host admin.</p>}
              </>}
              onJoin={() => void store.join(s.id)}
              onPlay={() => void store.play(s.id)}
              onDisconnect={() => void store.disconnect(s.id)}
              onCheck={() => void store.check(s.id)}
              onLaunch={() => void store.launch(s.id)}
              onOpenRequests={() => go("people")}
              manage={
                manage && {
                  onEdit: () => manage.onEdit(s),
                  onRemove: () => manage.onRemove(s),
                  onDiagnostics: () => manage.onDiagnostics(s),
                }
              }
            />
          </li>
        );
      })}
    </ul>
  );
}
