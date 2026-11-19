import { useState } from "react";
import { Button } from "../components/Button";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { CopyButton } from "../components/CopyButton";
import { ErrorState } from "../components/ErrorState";
import { Field, inputCls } from "../components/Field";
import { Header } from "../components/Header";
import { Modal } from "../components/Modal";
import { RequestInstructions } from "../components/RequestInstructions";
import { Skeleton } from "../components/Skeleton";
import { useNav } from "../lib/nav";
import { useAppState, useStore } from "../lib/store";
import type { HostedDevice, HostedGame, PendingDevice } from "../lib/types";
import { hostingError, useHostedGames, useLinkedGames } from "../lib/useHostedGames";

function shortKey(publicKey: string) {
  return publicKey.replace(/^nodekey:/, "").slice(0, 12) + "…";
}

function Timestamp({ value }: { value: string }) {
  return <time dateTime={value}>{new Date(value).toLocaleString()}</time>;
}

function PendingRequests({ pending, busy, onReview, onDismiss }: {
  pending: PendingDevice[];
  busy: boolean;
  onReview: (request: PendingDevice) => void;
  onDismiss?: (request: PendingDevice) => void;
}) {
  if (pending.length === 0) return null;
  return (
    <>
      <h4 className="mt-4 text-xs font-semibold text-muted">Incoming join requests</h4>
      <ul className="mt-1 divide-y divide-line">
        {pending.map((request) => (
          <li key={request.id} className="flex flex-wrap items-center justify-between gap-2 py-2">
            <p className="text-sm"><code>{shortKey(request.publicKey)}</code> · <span className="text-xs text-muted">Seen <Timestamp value={request.lastSeen} /></span></p>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" disabled={busy} onClick={() => onReview(request)}>Review request</Button>
              {onDismiss && <Button size="sm" variant="ghost" disabled={busy} onClick={() => onDismiss(request)}>Dismiss</Button>}
            </div>
          </li>
        ))}
      </ul>
    </>
  );
}

type GameForApproval = Pick<HostedGame, "id" | "name">;

export function PeopleScreen() {
  const { ready, identity } = useAppState();
  const { backend, retryLoad } = useStore();
  const { go } = useNav();
  const hosted = useHostedGames(backend);
  const linked = useLinkedGames(backend, 5000);
  const linkedGames = (linked.games ?? []).filter((game) => game.serverPublicKey);
  const [approval, setApproval] = useState<{ game: GameForApproval; request?: PendingDevice; serverPublicKey?: string }>();
  const [publicKey, setPublicKey] = useState("");
  const [label, setLabel] = useState("");
  const [revoking, setRevoking] = useState<{ game: HostedGame; device: HostedDevice }>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  async function act(action: () => Promise<void>, message: string, refresh = hosted.refresh) {
    setBusy(true);
    setNotice("");
    setError("");
    try {
      await action();
      await refresh();
      setNotice(message);
    } catch (e) {
      setError(hostingError(e));
    } finally {
      setBusy(false);
    }
  }

  function askApproval(game: GameForApproval, request?: PendingDevice, serverPublicKey?: string) {
    setPublicKey(request?.publicKey ?? "");
    setLabel("");
    setError("");
    setNotice("");
    setApproval({ game, request, serverPublicKey });
  }

  function approve() {
    if (busy || !approval || !publicKey.trim()) return;
    const { game, request, serverPublicKey } = approval;
    void act(async () => {
      if (request && serverPublicKey) {
        await backend.approveLinkedRequest(game.id, serverPublicKey, request, label.trim());
      } else if (request) {
        await backend.approveHostedRequest(game.id, request.id, label.trim());
      } else {
        await backend.approveHostedDevice(game.id, publicKey.trim(), label.trim());
      }
      setApproval(undefined);
    }, request ? `Device approved for ${game.name}. Ask your friend to try Join again.` : `Device approved for ${game.name}.`, serverPublicKey ? linked.refresh : hosted.refresh);
  }

  return (
    <>
      <Header title="People" subtitle="Approve devices for the games you host and share your own public key." />
      <div className="space-y-5">
        <section aria-labelledby="game-access" className="rounded-xl border border-line bg-surface p-5">
          <div className="flex items-start justify-between gap-3">
            <h2 id="game-access" className="font-semibold">Access to your hosted games</h2>
            <Button variant="ghost" size="sm" onClick={() => go("host")}>Go to Host</Button>
          </div>
          {(!approval && error) || hosted.error ? <p role="alert" className="mt-3 text-sm text-danger">{(!approval && error) || hosted.error}</p> : null}
          {notice && <p role="status" className="mt-3 text-sm text-accent">{notice}</p>}
          {hosted.games && (hosted.games.length === 0 ? <p className="mt-3 text-sm text-muted">No games hosted on this PC.</p> : (
            <div className="mt-2 divide-y divide-line">
              {hosted.games.map((game) => (
                <article key={game.id} aria-labelledby={`access-${game.id}`} className="py-4">
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <h3 id={`access-${game.id}`} className="min-w-0 font-medium [overflow-wrap:anywhere]">{game.name}</h3>
                    {game.paused && <span className="rounded bg-warn/15 px-1.5 py-0.5 text-xs text-warn">Sharing paused</span>}
                    <div className="flex flex-wrap gap-2">
                      <Button size="sm" disabled={busy} onClick={() => void act(() => backend.copyHostedInvitation(game.id), "Invitation copied. Share it privately.")}>Copy invitation</Button>
                      <Button size="sm" variant="primary" disabled={busy} onClick={() => askApproval(game)}>Approve device</Button>
                    </div>
                  </div>
                  <h4 className="mt-3 text-xs font-semibold text-muted">Approved devices</h4>
                  {game.approved.length === 0 ? <p className="mt-2 text-sm text-muted">No approved devices.</p> : (
                    <ul className="mt-1 divide-y divide-line">
                      {game.approved.map((device) => (
                        <li key={device.publicKey} className="flex flex-wrap items-center justify-between gap-2 py-2">
                          <div className="min-w-0">
                            <p className="text-sm [overflow-wrap:anywhere]">{device.label || "Unnamed device"} · <code>{shortKey(device.publicKey)}</code></p>
                            <p className="text-xs text-muted">Last connected {device.lastConnected ? <Timestamp value={device.lastConnected} /> : "· No connection recorded"}</p>
                          </div>
                          <Button size="sm" variant="ghost" disabled={busy} onClick={() => setRevoking({ game, device })}>Revoke</Button>
                        </li>
                      ))}
                    </ul>
                  )}
                  <PendingRequests pending={game.pending} busy={busy}
                    onReview={(request) => askApproval(game, request)}
                    onDismiss={(request) => void act(() => backend.dismissHostedRequest(game.id, request.id), "Join request dismissed.")}
                  />
                </article>
              ))}
            </div>
          ))}
          {!hosted.games && !hosted.error && <p role="status" className="mt-3 text-sm text-muted">Loading game access…</p>}
        </section>

        {(linkedGames.length > 0 || linked.error) && (
          <section aria-labelledby="linked-access" className="rounded-xl border border-line bg-surface p-5">
            <h2 id="linked-access" className="font-semibold">Access to games on your linked node</h2>
            {linked.error && <p role="alert" className="mt-3 text-sm text-danger">Could not read linked game access. {linked.error}</p>}
            <div className="mt-2 divide-y divide-line">
              {linkedGames.map((game) => (
                <article key={game.id} aria-labelledby={`linked-access-${game.id}`} className="py-4">
                  <h3 id={`linked-access-${game.id}`} className="font-medium [overflow-wrap:anywhere]">{game.name}</h3>
                  <p className="mt-1 text-xs text-muted">{game.approvedDevices} approved {game.approvedDevices === 1 ? "device" : "devices"}</p>
                  <PendingRequests pending={game.pending ?? []} busy={busy}
                    onReview={(request) => askApproval(game, request, game.serverPublicKey ?? undefined)}
                  />
                </article>
              ))}
            </div>
          </section>
        )}

        <section aria-labelledby="your-device" className="rounded-xl border border-line bg-surface p-5">
          <h2 id="your-device" className="font-semibold">Your public key</h2>
          <p className="mt-1 text-sm text-muted">Send this key to a game host so they can approve this device.</p>
          {!ready ? (
            <div role="status" aria-label="Loading device key" className="mt-4"><Skeleton className="h-16" /></div>
          ) : identity ? (
            <div className="mt-4 space-y-3">
              <div className="rounded-lg border border-line-strong bg-black/30 px-4 py-3">
                <code className="block font-mono text-sm leading-relaxed break-all select-all" aria-label="Public key">{identity.publicKey}</code>
              </div>
              <CopyButton text={identity.publicKey} label="Copy Public Key" variant="primary" />
            </div>
          ) : (
            <div className="mt-4"><ErrorState variant="inline" title="Could not read this device's key" onRetry={() => void retryLoad()} /></div>
          )}
        </section>
        {identity && <RequestInstructions publicKey={identity.publicKey} onImport={() => go("servers", { openImport: true })} />}
      </div>
      {approval && (
        <Modal title={`Approve a device for ${approval.game.name}`} onClose={() => { if (!busy) setApproval(undefined); }} footer={
          <>
            <Button variant="ghost" disabled={busy} onClick={() => setApproval(undefined)}>Cancel</Button>
            <Button variant="primary" loading={busy} disabled={!publicKey.trim()} onClick={approve}>Approve device</Button>
          </>
        }>
          <div className="space-y-4">
            <Field label="Device public key" hint="Check the full key with your friend before approving.">
              {({ id, describedBy }) => <input id={id} aria-describedby={describedBy} className={inputCls} value={publicKey} readOnly={Boolean(approval.request)} onChange={(e) => setPublicKey(e.target.value)} disabled={busy} autoComplete="off" spellCheck={false} />}
            </Field>
            <Field label="Label (optional)">
              {({ id }) => <input id={id} className={inputCls} value={label} onChange={(e) => setLabel(e.target.value)} disabled={busy} maxLength={64} />}
            </Field>
            {error && <p role="alert" className="text-sm text-danger">{error}</p>}
          </div>
        </Modal>
      )}
      {revoking && (
        <ConfirmDialog title="Revoke device access" message={`Revoke ${revoking.device.label || shortKey(revoking.device.publicKey)} from ${revoking.game.name}? Current sessions will close.`} confirmLabel="Revoke device"
          onClose={() => setRevoking(undefined)} onConfirm={() => {
            setRevoking(undefined);
            return act(() => backend.revokeHostedDevice(revoking.game.id, revoking.device.publicKey), `Device revoked from ${revoking.game.name}.`);
          }} />
      )}
    </>
  );
}
