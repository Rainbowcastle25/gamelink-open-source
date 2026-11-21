import { useEffect, useId, useState } from "react";
import { Button } from "./Button";
import { useStore } from "../lib/store";
import type { HostedGame, LanWorld } from "../lib/types";
import { hostingError } from "../lib/useHostedGames";

export function LanWorlds({ games, busy, showScanStatus = false, onShare }: {
  games: HostedGame[] | undefined;
  busy: boolean;
  showScanStatus?: boolean;
  onShare: (world: LanWorld) => Promise<void>;
}) {
  const { backend } = useStore();
  const headingId = useId();
  const [worlds, setWorlds] = useState<LanWorld[]>();
  const [scanError, setScanError] = useState("");

  useEffect(() => {
    let active = true;
    let timer: number;
    async function scan() {
      try {
        const found = await backend.scanLanWorlds();
        if (active) {
          setWorlds(found);
          setScanError("");
        }
      } catch (e) {
        if (active) {
          setWorlds(undefined);
          setScanError(hostingError(e));
        }
      } finally {
        if (active) timer = window.setTimeout(() => void scan(), 1000);
      }
    }
    void scan();
    return () => {
      active = false;
      window.clearTimeout(timer);
      void backend.stopLanScan().catch(() => {});
    };
  }, [backend]);

  const detected = (worlds ?? []).flatMap((world) => {
    const previous = games?.find((entry) => entry.lan && entry.name === world.name);
    const sharing = previous?.enabled && previous.port === world.port;
    return sharing && !showScanStatus ? [] : [{ world, previous, sharing }];
  });

  if (!showScanStatus && (!games || !detected.length)) return null;

  return (
    <section className="rounded-xl border border-line bg-surface p-5" aria-labelledby={headingId}>
      <h2 id={headingId} className="font-semibold">Minecraft Java LAN worlds</h2>
      {showScanStatus && (
        <p className="mt-1 text-sm text-muted">Open a world to LAN in Minecraft. GameLink looks for worlds announced by this PC on Dashboard and Host. Sharing pauses when a shared world closes.</p>
      )}
      {scanError ? <p role="alert" className="mt-3 text-sm text-danger">{scanError} You can enter the port below.</p> :
        detected.length ? (
          <ul className="mt-4 divide-y divide-line">
            {detected.map(({ world, previous, sharing }) => (
              <li key={JSON.stringify(world)} className="flex flex-wrap items-center justify-between gap-3 py-3">
                <div>
                  <p className="text-sm font-medium [overflow-wrap:anywhere]">{sharing ? world.name : `Share “${world.name}”?`}</p>
                  <p className="text-xs text-muted">{sharing ? "Sharing configured" : previous ? "Keep the same invitation and approved devices" : "Detected on this PC"} · Port {world.port}</p>
                </div>
                {!sharing && <Button variant="primary" size="sm" disabled={busy} onClick={() => void onShare(world)}>
                  {previous && previous.port !== world.port ? "Update LAN port" : "Share world"}
                </Button>}
              </li>
            ))}
          </ul>
        ) : <p className="mt-3 text-sm text-muted">{worlds ? "No LAN worlds detected yet." : "Starting LAN discovery…"}</p>}
    </section>
  );
}
