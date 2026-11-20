import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import type { Backend } from "../lib/backend";
import type { PanelConnection, PanelServer } from "../lib/types";
import { hostingError } from "../lib/useHostedGames";

export function PanelInventory({ backend }: { backend: Backend }) {
  const [panel, setPanel] = useState<PanelConnection | null>(null);
  const [servers, setServers] = useState<PanelServer[] | null>(null);
  const [error, setError] = useState("");
  const [refreshing, setRefreshing] = useState(false);

  useEffect(() => {
    let active = true;
    void backend.panelConnection().then((saved) => {
      if (active) setPanel(saved);
    }).catch((e: unknown) => {
      if (active) setError(hostingError(e));
    });
    return () => { active = false; };
  }, [backend]);

  async function refresh() {
    setRefreshing(true);
    setError("");
    try {
      const inventory = await backend.refreshPanelInventory();
      if (inventory === null) {
        setServers(null);
        setError("The saved Pterodactyl profile is missing. Restore it in the configuration folder, then refresh.");
      } else {
        setPanel(inventory.connection);
        setServers(inventory.servers);
      }
    } catch (e) {
      setError(hostingError(e));
    } finally {
      setRefreshing(false);
    }
  }

  if (!panel && !error) return null;

  return (
    <section className="mb-6 rounded-xl border border-line bg-surface p-5">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <h2 className="font-semibold">Pterodactyl inventory</h2>
          {panel && (
            <>
              <p className="mt-1 break-all text-sm text-muted">{panel.endpoint}</p>
              <p className="mt-1 text-sm text-muted">Saved nodes {panel.nodeIds.join(", ")}</p>
              <p className="mt-2 max-w-2xl text-sm text-muted">This inventory is read-only. To manage games and access, enter the GameLink agent endpoint and administrator token in Host connection. Saving a host keeps your Panel settings.</p>
            </>
          )}
        </div>
        {panel && <Button variant="secondary" loading={refreshing} disabled={refreshing} onClick={() => void refresh()}>Refresh inventory</Button>}
      </div>
      {error && <p role="alert" className="mt-3 text-sm text-danger">{error}</p>}
      {panel && servers && panel.nodeIds.map((id) => {
        const onNode = servers.filter((server) => server.nodeId === id);
        return (
          <div key={id} className="mt-4 rounded-lg border border-line px-4 py-3">
            <h3 className="text-sm font-semibold">Node {id}</h3>
            {onNode.length ? (
              <ul className="mt-2 list-inside list-disc text-sm text-muted">
                {onNode.map((server) => <li key={server.id}>{server.name}{server.suspended && " · Suspended"}</li>)}
              </ul>
            ) : <p className="mt-2 text-sm text-muted">No servers on this node.</p>}
          </div>
        );
      })}
    </section>
  );
}
