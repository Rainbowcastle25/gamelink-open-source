import { RefreshCw, Server, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { Button } from "../components/Button";
import { ActivityPanel } from "../components/ActivityPanel";
import { Header } from "../components/Header";
import { LanWorlds } from "../components/LanWorlds";
import { ServerList, useServerList } from "../components/ServerList";
import { SummarySkeleton } from "../components/Skeleton";
import { StatusIndicator } from "../components/StatusIndicator";
import { SummaryCard } from "../components/SummaryCard";
import { useAppState, useStore } from "../lib/store";
import { toast } from "../lib/toast";
import type { LanWorld } from "../lib/types";
import { hostingError } from "../lib/useHostedGames";

export function DashboardScreen() {
  const { ready, overview, servers } = useAppState();
  const store = useStore();
  const list = useServerList();
  const [sharing, setSharing] = useState(false);
  const realServers = servers.filter((s) => !s.demo).length;

  async function share(world: LanWorld) {
    setSharing(true);
    try {
      await store.backend.shareLanWorld(world);
      await list.local.refresh();
      toast.success(`Sharing configured for ${world.name}. Approve devices in People.`);
    } catch (error) {
      toast.error(hostingError(error));
    } finally {
      setSharing(false);
    }
  }

  return (
    <>
      <Header title="GameLink" subtitle="Private game servers for you and your friends">
        <StatusIndicator status={overview?.status ?? "idle"} />
      </Header>

      {!ready ? (
        <SummarySkeleton />
      ) : (
        <div className="grid grid-cols-2 gap-4">
          <SummaryCard icon={<Server className="size-4" aria-hidden />} label="Servers" value={overview || list.count > 0 ? list.count : "-"} note={`${overview?.connected ?? 0} connected`} />
          <SummaryCard icon={<ShieldCheck className="size-4" aria-hidden />} label="Security" value="Private" note="Only approved devices can connect." />
        </div>
      )}

      <div className="mt-8 grid items-start gap-6 min-[1100px]:grid-cols-[minmax(0,1fr)_300px]">
        <section aria-labelledby="your-servers" className="min-w-0">
          <div className="mb-3.5 flex items-center justify-between">
            <h2 id="your-servers" className="text-base font-semibold">
              Your Servers
            </h2>
            {realServers > 0 && (
              <Button variant="ghost" size="sm" onClick={() => void store.checkAll()} icon={<RefreshCw className="size-3.5" aria-hidden />}>
                Check all
              </Button>
            )}
          </div>
          <div className="space-y-3">
            <LanWorlds games={list.local.games} busy={sharing} onShare={share} />
            <ServerList list={list} />
          </div>
        </section>
        <ActivityPanel />
      </div>
    </>
  );
}
