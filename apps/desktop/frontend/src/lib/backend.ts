import type {
  ActivityEvent,
  CheckView,
  HostConnection,
  HostSnapshot,
  IdentityView,
  InvitePreview,
  LaunchResult,
  NewServerForm,
  Overview,
  Paths,
  ServerView,
  SettingsView,
  UpdateTopic,
  Version,
} from "./types";

/**
 * The part of the desktop service the UI uses. The UI is presentation only: behaviour lives behind the native command boundary. Two implementations exist:
 *  - realBackend.ts wraps the Tauri commands (production).
 *  - mockBackend.ts is for browser development, screenshots and unit tests.
 */
export interface Backend {
  version(): Promise<Version>;
  identity(): Promise<IdentityView>;
  paths(): Promise<Paths>;
  overview(): Promise<Overview>;
  servers(): Promise<ServerView[]>;
  settings(): Promise<SettingsView>;
  activity(limit: number): Promise<ActivityEvent[]>;
  clearActivity(): Promise<void>;

  /** Returns as soon as the attempt has begun; progress arrives via onUpdate / servers(). */
  join(id: string): Promise<void>;
  disconnect(id: string): Promise<void>;
  removeServer(id: string): Promise<void>;
  /** Starts a connected server's game; says whether it was told to join by itself. */
  launchGame(id: string): Promise<LaunchResult>;
  check(id: string): Promise<CheckView>;
  checkAll(): Promise<void>;
  diagnostics(id: string): Promise<string>;

  previewInvite(text: string): Promise<InvitePreview>;
  importInvite(text: string, localPort: number): Promise<ServerView>;
  addServer(form: NewServerForm): Promise<ServerView>;
  updateServer(id: string, name: string, description: string, localPort: number): Promise<ServerView>;
  updateSettings(v: SettingsView): Promise<SettingsView>;

  hostConnection(): Promise<HostConnection>;
  configureHost(endpoint: string, token: string): Promise<HostConnection>;
  hostStatus(): Promise<HostSnapshot>;
  refreshHost(): Promise<HostSnapshot>;
  importHostGame(gameID: string): Promise<ServerView>;

  copyText(text: string): Promise<void>;
  /** Native file dialog; resolves to "" when cancelled. */
  pickExecutable(title: string): Promise<string>;
  openConfigFolder(): Promise<void>;

  /** Subscribes to "gamelink:update"; returns an unsubscribe function. */
  onUpdate(cb: (topic: UpdateTopic) => void): () => void;
}

/**
 * True when the mock backend is active (VITE_MOCK=1, or ?mock=1 in dev mode).
 * Only used to show the "MOCK BACKEND" ribbon; the actual selection happens in
 * load() below so that production bundles contain no mock code at all.
 */
export const isMock: boolean =
  import.meta.env.VITE_MOCK === "1" ||
  (import.meta.env.DEV && typeof location !== "undefined" && new URLSearchParams(location.search).get("mock") === "1");

let loading: Promise<Backend> | null = null;

function load(): Promise<Backend> {
  if (!loading) {
    // Written so the bundler can drop the mock import from production builds:
    // both conditions are compile-time constants there (false).
    if (
      import.meta.env.VITE_MOCK === "1" ||
      (import.meta.env.DEV && typeof location !== "undefined" && new URLSearchParams(location.search).get("mock") === "1")
    ) {
      loading = import("./mockBackend").then((m) => m.createMockBackend());
    } else {
      loading = import("./realBackend").then((m) => m.realBackend);
    }
  }
  return loading;
}

function lazy<A extends unknown[], R>(pick: (b: Backend) => (...a: A) => Promise<R>) {
  return async (...a: A): Promise<R> => pick(await load())(...a);
}

/** The app-wide backend: resolves to the real bindings unless the mock is explicitly enabled. */
export const backend: Backend = {
  version: lazy((b) => b.version.bind(b)),
  identity: lazy((b) => b.identity.bind(b)),
  paths: lazy((b) => b.paths.bind(b)),
  overview: lazy((b) => b.overview.bind(b)),
  servers: lazy((b) => b.servers.bind(b)),
  settings: lazy((b) => b.settings.bind(b)),
  activity: lazy((b) => b.activity.bind(b)),
  clearActivity: lazy((b) => b.clearActivity.bind(b)),
  join: lazy((b) => b.join.bind(b)),
  disconnect: lazy((b) => b.disconnect.bind(b)),
  removeServer: lazy((b) => b.removeServer.bind(b)),
  launchGame: lazy((b) => b.launchGame.bind(b)),
  check: lazy((b) => b.check.bind(b)),
  checkAll: lazy((b) => b.checkAll.bind(b)),
  diagnostics: lazy((b) => b.diagnostics.bind(b)),
  previewInvite: lazy((b) => b.previewInvite.bind(b)),
  importInvite: lazy((b) => b.importInvite.bind(b)),
  addServer: lazy((b) => b.addServer.bind(b)),
  updateServer: lazy((b) => b.updateServer.bind(b)),
  updateSettings: lazy((b) => b.updateSettings.bind(b)),
  hostConnection: lazy((b) => b.hostConnection.bind(b)),
  configureHost: lazy((b) => b.configureHost.bind(b)),
  hostStatus: lazy((b) => b.hostStatus.bind(b)),
  refreshHost: lazy((b) => b.refreshHost.bind(b)),
  importHostGame: lazy((b) => b.importHostGame.bind(b)),
  copyText: lazy((b) => b.copyText.bind(b)),
  pickExecutable: lazy((b) => b.pickExecutable.bind(b)),
  openConfigFolder: lazy((b) => b.openConfigFolder.bind(b)),
  onUpdate(cb) {
    let off: (() => void) | undefined;
    let cancelled = false;
    void load().then((b) => {
      if (!cancelled) off = b.onUpdate(cb);
    });
    return () => {
      cancelled = true;
      off?.();
    };
  },
};
