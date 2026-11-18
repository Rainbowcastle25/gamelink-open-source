import { createContext, useContext, useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { backend as defaultBackend, type Backend } from "./backend";
import { errorMessage } from "./format";
import { toast } from "./toast";
import type { ActivityEvent, IdentityView, Overview, Paths, ServerView, SettingsView, Version } from "./types";

export type BusyKind = "joining" | "checking";

export interface AppState {
  /** The first load has finished (successfully or not). */
  ready: boolean;
  loadError: string | null;
  servers: ServerView[];
  overview: Overview | null;
  identity: IdentityView | null;
  paths: Paths | null;
  settings: SettingsView | null;
  version: Version | null;
  activity: ActivityEvent[];
  /** Per-server optimistic "something is happening" markers. */
  busy: Record<string, BusyKind>;
}

const initial: AppState = {
  ready: false,
  loadError: null,
  servers: [],
  overview: null,
  identity: null,
  paths: null,
  settings: null,
  version: null,
  activity: [],
  busy: {},
};

const ACTIVITY_LIMIT = 30;
// Lifecycle/path changes already arrive through backend events. This is only
// a low-frequency refresh for byte/session counters and missed notifications.
const POLL_MS = 15_000;
const LIVE_STATES = new Set(["connecting", "connected", "reconnecting"]);

/**
 * Small external store: holds what the Go side told us and nothing else.
 * Business rules stay in Go; this only fetches, caches and relays actions.
 */
export class GameLinkStore {
  private state: AppState = initial;
  private listeners = new Set<() => void>();
  private poll: ReturnType<typeof setInterval> | null = null;
  private active = false;
  private refreshing = false;
  private refreshAgain = false;
  /** Servers the player pressed Play on: launch the game once they connect. */
  private pendingPlay = new Set<string>();

  constructor(readonly backend: Backend) {}

  subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  getState = (): AppState => this.state;

  private set(patch: Partial<AppState>) {
    this.state = { ...this.state, ...patch };
    for (const l of [...this.listeners]) l();
    this.syncPoll();
  }

  private syncPoll() {
    const needed = this.active && this.state.servers.some((s) => LIVE_STATES.has(s.connection.state));
    if (needed && !this.poll) {
      this.poll = setInterval(() => void this.refreshServers(), POLL_MS);
    } else if (!needed && this.poll) {
      clearInterval(this.poll);
      this.poll = null;
    }
  }

  /** Starts listening for backend updates and loads everything. Returns a stop function. */
  start(): () => void {
    this.active = true;
    const off = this.backend.onUpdate((topic) => {
      if (topic === "servers") void this.refreshServers();
      else if (topic === "activity") void this.refreshActivity();
      else if (topic === "settings") void this.refreshSettings();
    });
    void this.loadAll();
    return () => {
      this.active = false;
      off();
      if (this.poll) clearInterval(this.poll);
      this.poll = null;
    };
  }

  async loadAll(): Promise<void> {
    const b = this.backend;
    const [version, identity, paths, settings, serversRes, activity] = await Promise.allSettled([
      b.version(),
      b.identity(),
      b.paths(),
      b.settings(),
      Promise.all([b.servers(), b.overview()]),
      b.activity(ACTIVITY_LIMIT),
    ]);
    if (!this.active) return;
    const patch: Partial<AppState> = { ready: true, loadError: null };
    if (version.status === "fulfilled") patch.version = version.value;
    if (identity.status === "fulfilled") patch.identity = identity.value;
    if (paths.status === "fulfilled") patch.paths = paths.value;
    if (settings.status === "fulfilled") patch.settings = settings.value;
    if (activity.status === "fulfilled") patch.activity = activity.value;
    if (serversRes.status === "fulfilled") {
      patch.servers = serversRes.value[0];
      patch.overview = serversRes.value[1];
    } else {
      patch.loadError = errorMessage(serversRes.reason);
    }
    this.set(patch);
  }

  async retryLoad(): Promise<void> {
    this.set({ ready: false, loadError: null });
    await this.loadAll();
  }

  async refreshServers(): Promise<void> {
    if (this.refreshing) {
      this.refreshAgain = true;
      return;
    }
    this.refreshing = true;
    try {
      do {
        this.refreshAgain = false;
        const [servers, overview] = await Promise.all([this.backend.servers(), this.backend.overview()]);
        if (!this.active) return;
        this.set({ servers, overview, loadError: null });
        this.firePendingPlays();
      } while (this.refreshAgain);
    } catch {
      // A failed background refresh is not worth interrupting the user; the
      // next event or poll tick retries.
    } finally {
      this.refreshing = false;
    }
  }

  async refreshActivity(): Promise<void> {
    try {
      const activity = await this.backend.activity(ACTIVITY_LIMIT);
      if (this.active) this.set({ activity });
    } catch {
      /* ignore, see refreshServers */
    }
  }

  async refreshSettings(): Promise<void> {
    try {
      const settings = await this.backend.settings();
      if (this.active) this.set({ settings });
    } catch {
      /* ignore */
    }
  }

  private setBusy(id: string, kind: BusyKind | null) {
    const busy = { ...this.state.busy };
    if (kind) busy[id] = kind;
    else delete busy[id];
    this.set({ busy });
  }

  // ---- server actions -------------------------------------------------------

  async join(id: string): Promise<void> {
    this.setBusy(id, "joining");
    try {
      await this.backend.join(id);
    } catch (e) {
      // A refused join cancels a pending Play: nothing is going to connect.
      this.pendingPlay.delete(id);
      toast.error(errorMessage(e));
    }
    await this.refreshServers();
    this.setBusy(id, null);
  }

  /**
   * One-click play: join, and start the game as soon as the connection is up.
   * A connection that fails keeps the request, so Retry still starts the game
   * once it connects; Disconnect or Dismiss cancels it. Auto-reconnects never
   * start the game a second time.
   */
  async play(id: string): Promise<void> {
    this.pendingPlay.add(id);
    await this.join(id);
  }

  /** Whether Play was pressed for this server and the game has not started yet. */
  isPlayPending(id: string): boolean {
    return this.pendingPlay.has(id);
  }

  // Only explicit signals cancel a pending Play (a refused join, Disconnect or
  // Dismiss, the server being removed): a "disconnected" snapshot can be stale.
  private firePendingPlays() {
    for (const id of [...this.pendingPlay]) {
      const s = this.state.servers.find((x) => x.id === id);
      if (!s) {
        this.pendingPlay.delete(id);
      } else if (s.connection.state === "connected") {
        this.pendingPlay.delete(id);
        void this.launch(id);
      }
    }
  }

  async disconnect(id: string): Promise<void> {
    this.pendingPlay.delete(id);
    try {
      await this.backend.disconnect(id);
    } catch (e) {
      toast.error(errorMessage(e));
    }
    await this.refreshServers();
  }

  async check(id: string): Promise<void> {
    this.setBusy(id, "checking");
    try {
      await this.backend.check(id);
    } catch (e) {
      toast.error(errorMessage(e));
    }
    await this.refreshServers();
    this.setBusy(id, null);
  }

  async checkAll(): Promise<void> {
    try {
      await this.backend.checkAll();
    } catch (e) {
      toast.error(errorMessage(e));
    }
    await this.refreshServers();
  }

  async launch(id: string): Promise<void> {
    try {
      const r = await this.backend.launchGame(id);
      if (r.autoConnect) {
        toast.success("Starting the game. It will join the server by itself.");
        return;
      }
      if (r.minecraftEntry) {
        toast.info(`Starting Minecraft. In Multiplayer, select “${r.minecraftEntry}”.`);
        return;
      }
      // The game cannot be told where to go (e.g. Minecraft's own launcher):
      // put the address on the clipboard so it is one paste away.
      let copied = false;
      try {
        await this.backend.copyText(r.address);
        copied = true;
      } catch {
        /* the toast still shows the address */
      }
      toast.info(`Starting the game. In it, join ${r.address}${copied ? " (copied, just paste it)" : ""}.`);
    } catch (e) {
      toast.error(errorMessage(e));
    }
  }

  async remove(id: string, name: string): Promise<boolean> {
    try {
      await this.backend.removeServer(id);
      toast.success(`Removed “${name}”`);
      await this.refreshServers();
      return true;
    } catch (e) {
      toast.error(errorMessage(e));
      return false;
    }
  }

  /** Callers show errors inline, so these throw. */
  async importInvite(text: string, localPort: number): Promise<ServerView> {
    const s = await this.backend.importInvite(text, localPort);
    await this.refreshServers();
    return s;
  }

  async addServer(form: Parameters<Backend["addServer"]>[0]): Promise<ServerView> {
    const s = await this.backend.addServer(form);
    await this.refreshServers();
    return s;
  }

  async updateServer(id: string, name: string, description: string, localPort: number): Promise<ServerView> {
    const s = await this.backend.updateServer(id, name, description, localPort);
    await this.refreshServers();
    return s;
  }

  // ---- settings & activity --------------------------------------------------

  /** Applies a settings change immediately (optimistic); reverts and reports on failure. */
  async updateSettings(patch: Partial<SettingsView>): Promise<boolean> {
    const before = this.state.settings;
    if (!before) return false;
    const next: SettingsView = { ...before, ...patch };
    this.set({ settings: next });
    try {
      const saved = await this.backend.updateSettings(next);
      this.set({ settings: saved });
      await this.refreshServers();
      return true;
    } catch (e) {
      this.set({ settings: before });
      toast.error(errorMessage(e));
      return false;
    }
  }

  async setExecutable(game: string, path: string): Promise<boolean> {
    const current = this.state.settings?.gameExecutables ?? {};
    const next = { ...current };
    if (path) next[game] = path;
    else delete next[game];
    return this.updateSettings({ gameExecutables: next });
  }

  /** Sets the arguments a game is started with ({host}, {port}, {address} are filled in). */
  async setLaunchArgs(game: string, args: string): Promise<boolean> {
    const current = this.state.settings?.gameLaunchArgs ?? {};
    if ((current[game] ?? "") === args) return true;
    return this.updateSettings({ gameLaunchArgs: { ...current, [game]: args } });
  }

  async clearActivity(): Promise<void> {
    try {
      await this.backend.clearActivity();
      this.set({ activity: [] });
    } catch (e) {
      toast.error(errorMessage(e));
    }
  }
}

const StoreContext = createContext<GameLinkStore | null>(null);

export function GameLinkProvider({ backend, children }: { backend?: Backend; children: ReactNode }) {
  const [store] = useState(() => new GameLinkStore(backend ?? defaultBackend));
  useEffect(() => store.start(), [store]);
  return <StoreContext.Provider value={store}>{children}</StoreContext.Provider>;
}

export function useStore(): GameLinkStore {
  const s = useContext(StoreContext);
  if (!s) throw new Error("GameLinkProvider is missing");
  return s;
}

export function useAppState(): AppState {
  const store = useStore();
  return useSyncExternalStore(store.subscribe, store.getState, store.getState);
}
