import { useState } from "react";
import { Button } from "../components/Button";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { Field, inputCls } from "../components/Field";
import { Header } from "../components/Header";
import { LanWorlds } from "../components/LanWorlds";
import { GAME_OPTIONS } from "../lib/games";
import { useNav } from "../lib/nav";
import { useStore } from "../lib/store";
import type { DesktopGameType, HostedGame, LanWorld } from "../lib/types";
import { hostingError, useHostedGames } from "../lib/useHostedGames";

const supported = GAME_OPTIONS.filter((game) => game.id !== "custom");

export function LocalHostScreen() {
  const { backend } = useStore();
  const { go } = useNav();
  const hosted = useHostedGames(backend);
  const [game, setGame] = useState<DesktopGameType>("minecraft-java");
  const [port, setPort] = useState("25565");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [createdId, setCreatedId] = useState("");
  const [removing, setRemoving] = useState<HostedGame>();

  async function act(action: () => Promise<void>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await action();
      await hosted.refresh();
    } catch (e) {
      setError(hostingError(e));
    } finally {
      setBusy(false);
    }
  }

  function create(world?: LanWorld) {
    return act(async () => {
      const result = world
        ? await backend.shareLanWorld(world)
        : await backend.createHostedGame({ name: name.trim(), game, port: Number(port) });
      setCreatedId(result.id);
      setNotice(`Sharing configured for ${world?.name ?? name.trim()}. Approve devices in People.`);
      if (!world) setName("");
    });
  }

  function copy(id: string) {
    return act(async () => {
      await backend.copyHostedInvitation(id);
      setNotice("Invitation copied. Share it privately.");
    });
  }

  return (
    <>
      <Header title="Host" subtitle="Share a game running on this PC with approved devices." />
      <div className="space-y-5">
        <LanWorlds games={hosted.games} busy={busy} showScanStatus onShare={create} />

        <section className="rounded-xl border border-line bg-surface p-5" aria-labelledby="manual-host">
          <h2 id="manual-host" className="font-semibold">Host a server on this PC</h2>
          <p className="mt-1 text-sm text-muted">Start the game server first and enter its local port.</p>
          <form className="mt-4 grid gap-4 md:grid-cols-[1fr_1fr_120px_auto] md:items-end" onSubmit={(e) => {
            e.preventDefault();
            if (!busy) void create();
          }}>
            <Field label="Server name">
              {({ id }) => <input id={id} className={inputCls} value={name} onChange={(e) => setName(e.target.value)} required maxLength={64} />}
            </Field>
            <Field label="Game">
              {({ id }) => (
                <select id={id} className={inputCls} value={game} onChange={(e) => {
                  const selected = supported.find((entry) => entry.id === e.target.value);
                  if (!selected || selected.id === "custom") return;
                  setGame(selected.id);
                  setPort(String(selected.defaultPort));
                }}>
                  {supported.map((entry) => <option key={entry.id} value={entry.id}>{entry.label}</option>)}
                </select>
              )}
            </Field>
            <Field label="Local port">
              {({ id }) => <input id={id} className={inputCls} type="number" min={1} max={65535} value={port} onChange={(e) => setPort(e.target.value)} required />}
            </Field>
            <Button type="submit" variant="primary" disabled={busy || !name.trim()}>Host server</Button>
          </form>
        </section>

        {(error || hosted.error) && <p role="alert" className="text-sm text-danger">{error || hosted.error}</p>}
        {notice && (
          <div className="flex flex-wrap items-center gap-3 rounded-lg border border-teal/25 bg-teal/8 p-3 text-sm text-accent">
            <p role="status">{notice}</p>
            {createdId && <Button size="sm" disabled={busy} onClick={() => void copy(createdId)}>Copy invitation</Button>}
          </div>
        )}
        <section className="rounded-xl border border-line bg-surface p-5" aria-labelledby="hosted-games">
          <div className="flex items-center justify-between gap-3">
            <h2 id="hosted-games" className="font-semibold">Your hosted games</h2>
            {!!hosted.games?.length && <Button variant="ghost" size="sm" onClick={() => go("people")}>Manage access</Button>}
          </div>
          {hosted.games && (hosted.games.length === 0 ? <p className="mt-3 text-sm text-muted">No games hosted on this PC.</p> : (
            <ul className="mt-3 divide-y divide-line">
              {hosted.games.map((entry) => (
                <li key={entry.id} className="flex flex-wrap items-center justify-between gap-3 py-3">
                  <div>
                    <h3 className="text-sm font-medium [overflow-wrap:anywhere]">{entry.name}</h3>
                    {entry.paused && <span className="rounded bg-warn/15 px-1.5 py-0.5 text-xs text-warn">Sharing paused</span>}
                    <p className="text-xs text-muted">Port {entry.port} · {entry.approved.length} approved · {entry.state === "serving" ? "Tunnel active" : entry.detail}</p>
                  </div>
                  <div className="flex flex-wrap gap-2">
                    {(entry.enabled || !entry.lan) && (
                      <Button size="sm" disabled={busy} onClick={() => void act(() => backend.setHostingEnabled(entry.id, !entry.enabled))}>
                        {entry.enabled ? "Stop sharing" : "Start sharing"}
                      </Button>
                    )}
                    <Button size="sm" variant="ghost" disabled={busy} onClick={() => setRemoving(entry)}>Remove</Button>
                  </div>
                </li>
              ))}
            </ul>
          ))}
        </section>
      </div>
      {removing && <ConfirmDialog title="Remove hosted game" message={`Remove ${removing.name}? This ends active sessions and invalidates the invitation.`} confirmLabel="Remove game" onClose={() => setRemoving(undefined)} onConfirm={() => act(async () => {
        await backend.removeHostedGame(removing.id);
        if (createdId === removing.id) setCreatedId("");
        setRemoving(undefined);
      })} />}
    </>
  );
}
