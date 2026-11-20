import { useEffect, useState } from "react";
import { Clock3, Database, RefreshCw, Server, ShieldCheck } from "lucide-react";
import { Button } from "../components/Button";
import { Field, inputCls } from "../components/Field";
import { Header } from "../components/Header";
import { useStore } from "../lib/store";
import type { HostConnection, HostSnapshot } from "../lib/types";
import { LEGACY_AGENT_UPDATE } from "../lib/useHostedGames";

function errorText(error: unknown) {
  if (typeof error === "string") return error;
  return error instanceof Error ? error.message : "The host request failed";
}

function stateLabel(state: string) {
  if (!state || state === "unknown") return "Unknown";
  return state.charAt(0).toUpperCase() + state.slice(1);
}

export function HostScreen() {
  const store = useStore();
  const backend = store.backend;
  const [connection, setConnection] = useState<HostConnection | null>(null);
  const [endpoint, setEndpoint] = useState("");
  const [token, setToken] = useState("");
  const [snapshot, setSnapshot] = useState<HostSnapshot | null>(null);
  const [loadingConnection, setLoadingConnection] = useState(true);
  const [saving, setSaving] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [importing, setImporting] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const actionBusy = saving || refreshing || importing !== null;

  // Opening this screen reads local connection metadata only. Network requests
  // happen only when the user presses Refresh or Add to GameLink.
  useEffect(() => {
    let active = true;
    void backend.hostConnection().then((c) => {
      if (!active) return;
      setConnection(c);
      setEndpoint(c.endpoint);
    }).catch((e: unknown) => {
      if (active) setError(errorText(e));
    }).finally(() => {
      if (active) setLoadingConnection(false);
    });
    return () => { active = false; };
  }, [backend]);

  async function save() {
    setSaving(true);
    setError("");
    setNotice("");
    try {
      const c = await store.configureHost(endpoint, token);
      setConnection(c);
      setEndpoint(c.endpoint);
      setToken("");
      setSnapshot(null);
      setNotice("Host connection saved on this device.");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setSaving(false);
    }
  }

  async function refresh() {
    setRefreshing(true);
    setError("");
    setNotice("");
    try {
      setSnapshot(await backend.refreshHost());
    } catch (e) {
      setError(errorText(e));
    } finally {
      setRefreshing(false);
    }
  }

  async function addGame(id: string, serverFingerprint: string) {
    setImporting(id);
    setError("");
    setNotice("");
    try {
      const server = await backend.importHostGame(id, serverFingerprint);
      await store.refreshServers();
      setNotice(`${server.name} was added to your servers.`);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setImporting(null);
    }
  }

  return (
    <>
      <Header title="Host admin" subtitle="Discover games on your GameLink host and add this device when you choose." />

      <section className="mb-6 rounded-xl border border-line bg-surface p-5">
        <div className="mb-4 flex items-start gap-3">
          <div className="grid size-10 shrink-0 place-items-center rounded-lg bg-teal/10 text-accent"><ShieldCheck className="size-5" aria-hidden /></div>
          <div>
            <h2 className="font-semibold">Host connection</h2>
            <p className="mt-1 max-w-2xl text-sm leading-relaxed text-muted">Use the host's HTTPS address and administrator token. The token is protected in this device's private config and is cleared from this form after saving.</p>
          </div>
        </div>
        <div className="grid gap-4 md:grid-cols-[1.3fr_1fr_auto] md:items-end">
          <Field label="Host endpoint" hint="For SSH forwarding, use a literal loopback address such as http://127.0.0.1:8467.">
            {({ id, describedBy }) => <input id={id} aria-describedby={describedBy} className={inputCls} value={endpoint} onChange={(e) => setEndpoint(e.target.value)} placeholder="https://host.example.com" autoComplete="url" disabled={actionBusy} />}
          </Field>
          <Field label="Administrator token" hint={connection?.configured ? "Leave blank to keep the saved token for this same endpoint." : "The token is never shown again after saving."}>
            {({ id, describedBy }) => <input id={id} aria-describedby={describedBy} className={inputCls} type="password" value={token} onChange={(e) => setToken(e.target.value)} placeholder={connection?.configured ? "Saved securely" : "Paste host admin token"} autoComplete="new-password" disabled={actionBusy} />}
          </Field>
          <Button variant="primary" loading={saving || loadingConnection} onClick={() => void save()} disabled={loadingConnection || actionBusy || !endpoint.trim()}>Save connection</Button>
        </div>
        {connection?.configured && <p className="mt-3 text-xs text-muted">Connected host: <span className="font-mono text-fg">{connection.endpoint}</span></p>}
      </section>

      <section className="rounded-xl border border-line bg-surface p-5">
        <div className="mb-4 flex flex-wrap items-center justify-between gap-4">
          <div>
            <h2 className="font-semibold">Discovered games</h2>
            <p className="mt-1 text-sm text-muted">Refresh registers the host and samples each game's current status. Nothing updates in the background.</p>
          </div>
          <Button icon={<RefreshCw className="size-4" aria-hidden />} variant="secondary" loading={refreshing} disabled={!connection?.configured || actionBusy} onClick={() => void refresh()}>
            Refresh games
          </Button>
        </div>

        {error && <div role="alert" className="mb-4 rounded-lg border border-danger/30 bg-danger/8 px-4 py-3 text-sm text-danger">{error}</div>}
        {notice && <div role="status" className="mb-4 rounded-lg border border-teal/25 bg-teal/8 px-4 py-3 text-sm text-accent">{notice}</div>}

        {!connection?.configured ? (
          <div className="rounded-lg border border-dashed border-line-strong px-5 py-8 text-center text-sm text-muted">Save a host connection to discover its games.</div>
        ) : !snapshot ? (
          <div className="rounded-lg border border-dashed border-line-strong px-5 py-8 text-center">
            <Database className="mx-auto mb-2 size-5 text-faint" aria-hidden />
            <p className="text-sm text-muted">Select Refresh games to discover servers and sample their status.</p>
          </div>
        ) : snapshot.games.length === 0 ? (
          <div className="rounded-lg border border-dashed border-line-strong px-5 py-8 text-center text-sm text-muted">No GameLink games were found on this host.</div>
        ) : (
          <div className="divide-y divide-line rounded-lg border border-line">
            {snapshot.games.map((game) => (
              <article key={game.id} className="flex flex-wrap items-center justify-between gap-4 px-4 py-4">
                <div className="flex min-w-0 items-start gap-3">
                  <Server className="mt-0.5 size-4 shrink-0 text-faint" aria-hidden />
                  <div className="min-w-0">
                    <h3 className="truncate text-sm font-medium">{game.name}</h3>
                    <p className="mt-1 text-xs text-muted">{game.type} · {game.approvedDevices} approved {game.approvedDevices === 1 ? "device" : "devices"}{game.disabled ? " · disabled" : ""}</p>
                    <p className="mt-1 text-xs text-muted">Status: <span className={game.state === "unknown" ? "text-muted" : "text-fg"}>{stateLabel(game.state)}</span>{game.detail ? ` · ${game.detail}` : ""}</p>
                    {game.legacyAgent && <p className="mt-1 text-sm text-muted">{LEGACY_AGENT_UPDATE}</p>}
                  </div>
                </div>
                <Button variant="primary" size="sm" loading={importing === game.id} disabled={actionBusy || game.disabled || game.legacyAgent || !game.serverFingerprint} onClick={() => void addGame(game.id, game.serverFingerprint ?? "")}>Add to GameLink</Button>
              </article>
            ))}
          </div>
        )}
        {snapshot?.refreshedAt && <p className="mt-3 flex items-center gap-1.5 text-xs text-faint"><Clock3 className="size-3.5" aria-hidden />Status sampled {new Date(snapshot.refreshedAt).toLocaleString()}</p>}
        {snapshot && <p className="mt-2 text-xs text-muted">Unknown means the host could not determine a status. It does not confirm that a game server is offline.</p>}
        {snapshot && <p className="mt-2 text-xs text-muted">Add to GameLink explicitly approves this device on the selected game and imports it to your Servers list.</p>}
      </section>
    </>
  );
}
