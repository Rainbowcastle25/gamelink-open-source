import { FileInput, Plus, Server as ServerIcon } from "lucide-react";
import { useNav } from "../lib/nav";
import { useAppState, useStore } from "../lib/store";
import type { ServerView } from "../lib/types";
import { Button } from "./Button";
import { EmptyState } from "./EmptyState";
import { ErrorState } from "./ErrorState";
import { ServerCard } from "./ServerCard";
import { ServerListSkeleton } from "./Skeleton";

export interface ManageHandlers {
  onEdit: (s: ServerView) => void;
  onRemove: (s: ServerView) => void;
  onDiagnostics: (s: ServerView) => void;
}

/** The vertical list of server cards, wired to the store. Also owns the loading / error / empty states. */
export function ServerList({ manage }: { manage?: ManageHandlers }) {
  const { ready, loadError, servers, busy } = useAppState();
  const store = useStore();
  const { go } = useNav();

  if (!ready) return <ServerListSkeleton />;

  if (loadError && servers.length === 0) {
    return (
      <ErrorState
        title="Could not load your servers"
        message={`${loadError} If this keeps happening, restart GameLink.`}
        onRetry={() => void store.retryLoad()}
      />
    );
  }

  if (servers.length === 0) {
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
      {servers.map((s) => (
        <li key={s.id} className="min-w-0">
          <ServerCard
            server={s}
            busy={busy[s.id]}
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
      ))}
    </ul>
  );
}
