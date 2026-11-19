// MOCK BACKEND. Used only for browser development (VITE_MOCK=1 or ?mock=1 in
// dev), screenshots and unit tests. Production builds never include this file:
// see load() in backend.ts. The data below is fictional.
import { Kind } from "./types";
import { Protocol, Type } from "../lib/types";
import type { Backend } from "./backend";
import type {
  ActivityEvent,
  CheckView,
  ConnectionView,
  InvitePreview,
  NewServerForm,
  ServerView,
  LaunchResult,
  HostConnection,
  HostSnapshot,
  HostedGame,
  HostedGameForm,
  SettingsView,
  UpdateTopic,
} from "./types";

/** Mirrors games.Info.LaunchArgs / LaunchURI on the Go side. */
const DEFAULT_LAUNCH_ARGS: Record<string, string> = { "minecraft-java": "", "minecraft-bedrock": "", factorio: "--mp-connect {address}", custom: "" };
const BEDROCK_URI = "minecraft://connect?serverUrl={host}&serverPort={port}";
const expandLaunch = (t: string, port: number) =>
  t.replaceAll("{host}", "127.0.0.1").replaceAll("{port}", String(port)).replaceAll("{address}", `127.0.0.1:${port}`);
const hasPlaceholder = (t: string) => /\{(host|port|address)\}/.test(t);

export interface MockOptions {
  /** Latency added to every call (ms). Tests use 0. */
  delayMs?: number;
  /** How long "connecting" lasts before the outcome is reached (ms). */
  joinMs?: number;
  /** Start with the sample servers (default true). */
  seed?: boolean;
  /** Start with the sample activity (default true, only if seeded). */
  demoMode?: boolean;
}

export type MockBackend = Backend & {
  /** Test helper: push a topic to subscribers as the Go side would. */
  emit(topic: UpdateTopic): void;
  /** Test helper: replace/patch a server and notify. */
  patchServer(id: string, patch: Partial<ServerView>): void;
  /** Test helper: every game launch asked for (program and args, or URI). */
  launches: Array<{ id: string; target: string }>;
};

const PUBLIC_RELAY = ["relay.tailcat.example"];
const MOCK_KEY = "nodekey:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const GAME_INFO: Record<string, { name: string; proto: Protocol; port: number; hint: string }> = {
  "minecraft-java": {
    name: "Minecraft: Java Edition",
    proto: Protocol.TCP,
    port: 25565,
    hint: "In Minecraft choose Multiplayer, then Add Server, and enter the local address shown here as the server address.",
  },
  "minecraft-bedrock": {
    name: "Minecraft: Bedrock Edition",
    proto: Protocol.UDP,
    port: 19132,
    hint: "In Minecraft Bedrock choose Play, Servers, Add Server and enter 127.0.0.1 with the port shown here. Bedrock on consoles and phones is not supported.",
  },
  factorio: {
    name: "Factorio",
    proto: Protocol.UDP,
    port: 34197,
    hint: "In Factorio choose Multiplayer, Connect to address, and enter the local address shown here.",
  },
  custom: { name: "Other game", proto: Protocol.$zero, port: 0, hint: "Point your game at the local address shown here." },
};

const idle = (): ConnectionView => ({
  state: "disconnected",
  message: "",
  localAddr: "",
  localPort: 0,
  portFallback: false,
  path: "unknown",
  relay: "",
  latencyMs: null,
  gameReachable: "unknown",
  since: "",
  activeConns: 0,
  udpFlows: 0,
  reconnects: 0,
  oversizeDrops: 0,
  bytesToServer: 0,
  bytesFromServer: 0,
  simulated: false,
});

const unchecked = (): CheckView => ({ state: "unknown", message: "Not checked yet", latencyMs: null, path: "", checkedAt: "" });

function iso(msAgo = 0): string {
  return new Date(Date.now() - msAgo).toISOString();
}

function makeServer(p: Partial<ServerView> & Pick<ServerView, "id" | "name" | "game">): ServerView {
  const info = GAME_INFO[p.game] ?? GAME_INFO.custom;
  return {
    description: "",
    gameName: info.name,
    proto: info.proto,
    port: info.port,
    localPort: 0,
    defaultPort: info.port,
    addedAt: iso(86_400_000 * 3),
    relayHosts: PUBLIC_RELAY,
    defaultRelay: true,
    serverKey: "nodekey:b7d1f40c9a2e5683d0c4a1f79e28b563",
    hint: info.hint,
    hasExecutable: p.game === Type.MinecraftBedrock,
    demo: false,
    connection: idle(),
    check: unchecked(),
    ...p,
  };
}

function seedServers(): ServerView[] {
  return [
    makeServer({
      id: "mock-survival",
      name: "Friends Survival",
      description: "Weekend world with the usual crew",
      game: Type.MinecraftJava,
      hasExecutable: true,
      check: { state: "online", message: "Server answered", latencyMs: 28, path: "direct", checkedAt: iso(120_000) },
    }),
    makeServer({
      id: "mock-bedrock",
      name: "Creative Realm",
      description: "Build contests on Fridays",
      game: Type.MinecraftBedrock,
    }),
    makeServer({
      id: "mock-factorio",
      name: "Mega Factory",
      description: "Shared Factorio run, 2.0",
      game: Type.Factorio,
      serverKey: "nodekey:0c93e5a1d7f2486b9e1ac5d03f7b2e64",
      check: { state: "online", message: "Server answered", latencyMs: 63, path: "relay", checkedAt: iso(300_000) },
    }),
    makeServer({
      id: "mock-modded",
      name: "Modded Playground",
      description: "Heavy modpack, custom relay",
      game: Type.MinecraftJava,
      relayHosts: ["relay.lab.internal"],
      defaultRelay: false,
      serverKey: "nodekey:a14e7c0b92d5f3861e0ab47d5c9f2038",
      check: {
        state: "unreachable",
        message: "No response: the server may be offline, or this device is not approved",
        latencyMs: null,
        path: "",
        checkedAt: iso(900_000),
      },
    }),
  ];
}

function demoServers(): ServerView[] {
  const sample = (id: string, name: string, game: Type) =>
    makeServer({
      id,
      name,
      game,
      description: "Sample server for trying the interface",
      demo: true,
      check: { state: "unknown", message: "Demo servers are not real", latencyMs: null, path: "", checkedAt: "" },
    });
  return [
    sample("demo-mc", "Demo: Survival World", Type.MinecraftJava),
    sample("demo-bedrock", "Demo: Creative Realm", Type.MinecraftBedrock),
    sample("demo-factorio", "Demo: Mega Factory", Type.Factorio),
  ];
}

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

export function createMockBackend(opts: MockOptions = {}): MockBackend {
  const delayMs = opts.delayMs ?? 120;
  const joinMs = opts.joinMs ?? 1000;
  const seed = opts.seed ?? true;

  let saved: ServerView[] = seed ? seedServers() : [];
  const world = { name: "Demo: LAN Cabin", port: 51734 };
  let hosted: HostedGame[] = seed ? [{
    id: "demo-hosted-factory", serverFingerprint: "f".repeat(64), game: Type.Factorio, name: "Demo: Factory", port: 34197,
    enabled: true, lan: false, state: "serving", detail: "",
    approved: [{ publicKey: "nodekey:" + "a".repeat(64), label: "Demo: Alex", lastConnected: iso(80000) }],
    pending: [{ id: 1, publicKey: "nodekey:" + "b".repeat(64), lastSeen: iso(10000) }],
  }] : [];
  let nextHosted = 1;

  function hostedGame(id: string): HostedGame {
    const game = hosted.find((game) => game.id === id);
    if (!game) throw new Error("Hosted game was removed");
    return game;
  }

  function updateHosting(game: HostedGame) {
    game.state = game.enabled && game.approved.length > 0 ? "serving" : "inactive";
    game.detail = !game.enabled ? "Sharing paused" : game.state === "inactive" ? "waiting for join requests; no approved devices" : "";
    if (!game.enabled) game.pending = [];
  }

  function pendingRequest(game: HostedGame, requestId: number) {
    const request = game.pending.find((request) => request.id === requestId);
    if (!request) throw new Error("That join request is no longer pending");
    return request;
  }

  function approveDevice(game: HostedGame, publicKey: string, label: string) {
    if (!/^nodekey:[0-9a-f]{64}$/i.test(publicKey)) throw new Error("Enter a valid device public key");
    const device = game.approved.find((device) => device.publicKey === publicKey);
    if (device) device.label = label || device.label;
    else game.approved.push({ publicKey, label, lastConnected: null });
    game.pending = game.pending.filter((request) => request.publicKey !== publicKey);
    updateHosting(game);
  }

  function createHosted(form: HostedGameForm, lan: boolean) {
    const game: HostedGame = {
      ...form, id: `demo-hosted-${nextHosted}`, serverFingerprint: (nextHosted++).toString(16).padStart(64, "0"), enabled: true, lan,
      state: "inactive", detail: "", approved: [], pending: [],
    };
    updateHosting(game);
    hosted.push(game);
    return { id: game.id };
  }
  const live = new Map<string, ServerView>(); // demo servers keep their state here
  let settings: SettingsView = {
    demoMode: opts.demoMode ?? false,
    autoReconnect: true,
    checkOnStartup: true,
    checkForUpdates: false,
    canCheckForUpdates: true,
    verboseLogging: false,
    allowLocalRelay: false,
    minecraftServerList: false,
    prismInstancesDir: "",
    presenceNotifications: false,
    minecraftSyncError: "",
    gameExecutables: seed ? { "minecraft-java": "C:\\Games\\Minecraft\\Minecraft Launcher.exe" } : {},
    gameLaunchArgs: { ...DEFAULT_LAUNCH_ARGS },
  };
  let activity: ActivityEvent[] = seed
    ? [
        { time: iso(95_000), kind: Kind.KindConnect, serverId: "mock-survival", message: "Connected to “Friends Survival” at 127.0.0.1:25565 (direct, 34 ms)" },
        { time: iso(14 * 60_000), kind: Kind.KindWarning, serverId: "mock-modded", message: "Could not connect to “Modded Playground”: No response from the server" },
        { time: iso(3 * 3_600_000), kind: Kind.KindInfo, message: "Added server “Mega Factory”" },
        { time: iso(26 * 3_600_000), kind: Kind.KindInfo, message: "Created this device's identity" },
      ]
    : [];
  let nextId = 1;
  let hostEndpoint = "";
  let hostSnapshot: HostSnapshot = { games: [], refreshedAt: "" };
  const listeners = new Set<(t: UpdateTopic) => void>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const launches: Array<{ id: string; target: string }> = [];

  const emit = (t: UpdateTopic) => {
    for (const l of [...listeners]) l(t);
  };
  const wait = (): Promise<void> => (delayMs > 0 ? new Promise((r) => setTimeout(r, delayMs)) : Promise.resolve());
  const find = (id: string): ServerView | undefined => {
    const s = saved.find((x) => x.id === id);
    if (s) return s;
    if (!id.startsWith("demo-") || !settings.demoMode) return undefined;
    let d = live.get(id);
    if (!d) {
      d = demoServers().find((x) => x.id === id);
      if (d) live.set(id, d);
    }
    return d;
  };
  const log = (kind: Kind, message: string, serverId?: string) => {
    activity = [{ time: iso(), kind, message, serverId }, ...activity].slice(0, 200);
    emit("activity");
  };
  const setConn = (s: ServerView, patch: Partial<ConnectionView>) => {
    s.connection = { ...s.connection, ...patch };
    emit("servers");
  };
  const schedule = (id: string, ms: number, fn: () => void) => {
    const prev = timers.get(id);
    if (prev) clearTimeout(prev);
    timers.set(
      id,
      setTimeout(() => {
        timers.delete(id);
        fn();
      }, ms),
    );
  };

  const allServers = (): ServerView[] => {
    const out = [...saved];
    if (settings.demoMode) {
      for (const d of demoServers()) out.push(live.get(d.id) ?? d);
    }
    return out;
  };

  const pathLabel = (c: ConnectionView) => `${c.path === "relay" ? "relayed" : "direct"}${c.latencyMs != null ? `, ${Math.round(c.latencyMs)} ms` : ""}`;

  function runJoin(s: ServerView) {
    const base = GAME_INFO[s.game]?.port ?? 0;
    setConn(s, { state: "connecting", message: "Connecting…" });
    log(Kind.KindInfo, `Connecting to “${s.name}”…`, s.id);
    schedule(s.id, joinMs, () => {
      const since = iso();
      if (s.demo) {
        setConn(s, {
          state: "connected",
          message: "Simulated connection: no network traffic is sent in demo mode",
          localAddr: `127.0.0.1:${base}`,
          localPort: base,
          since,
          simulated: true,
        });
        return;
      }
      switch (s.id) {
        case "mock-bedrock":
          setConn(s, {
            state: "unreachable",
            message: "No response from the server: it may be offline, or this device has not been approved for this game",
          });
          log(Kind.KindWarning, `Could not connect to “${s.name}”: No response from the server`, s.id);
          return;
        case "mock-modded":
          setConn(s, { state: "error", message: "Could not reach the relay relay.lab.internal: connection refused" });
          log(Kind.KindError, `Could not connect to “${s.name}”: connection refused`, s.id);
          return;
        case "mock-factorio":
          setConn(s, {
            state: "connected",
            message: "",
            localAddr: "127.0.0.1:34198",
            localPort: 34198,
            portFallback: true,
            path: "relay",
            relay: "relay.tailcat.example",
            latencyMs: 71,
            gameReachable: "no",
            since,
            udpFlows: 1,
            oversizeDrops: 3,
            reconnects: 1,
            bytesToServer: 18_200,
            bytesFromServer: 9_400,
          });
          break;
        default: {
          const port = s.localPort || base || 25565;
          setConn(s, {
            state: "connected",
            message: "",
            localAddr: `127.0.0.1:${port}`,
            localPort: port,
            path: "direct",
            relay: "",
            latencyMs: 34,
            gameReachable: "yes",
            since,
            activeConns: 0,
          });
        }
      }
      s.check = s.connection.gameReachable === "no"
        ? { state: "offline", message: "Game server is not responding", latencyMs: s.connection.latencyMs, path: s.connection.path, checkedAt: iso() }
        : s.connection.gameReachable === "yes"
          ? { state: "online", message: "Game server answered", latencyMs: s.connection.latencyMs, path: s.connection.path, checkedAt: iso() }
          : { state: "unknown", message: "Agent online, game status unknown", latencyMs: s.connection.latencyMs, path: s.connection.path, checkedAt: iso() };
      log(Kind.KindConnect, `Connected to “${s.name}” at ${s.connection.localAddr} (${pathLabel(s.connection)})`, s.id);
    });
  }

  function previewOf(text: string): InvitePreview {
    const t = text.trim();
    if (!t.startsWith("glink1.")) {
      throw new Error('this does not look like a GameLink invitation (it should start with "glink1.")');
    }
    if (t.length < 14) throw new Error("this invitation is incomplete or damaged; ask the owner to send it again");
    const lower = t.toLowerCase();
    const game = lower.includes("factorio") ? "factorio" : lower.includes("bedrock") ? "bedrock" : "minecraft-java";
    const gid = game === "bedrock" ? "minecraft-bedrock" : game;
    const info = GAME_INFO[gid];
    const custom = lower.includes("custom");
    return {
      name: gid === "factorio" ? "Weekend Factory" : gid === "minecraft-bedrock" ? "Bedrock Hangout" : "Weekend Minecraft",
      game: gid as Type,
      gameName: info.name,
      proto: info.proto,
      port: info.port,
      note: "Join whenever, the world is up most evenings.",
      relayHosts: custom ? ["relay.lab.internal"] : PUBLIC_RELAY,
      relayRegion: custom ? "" : "eu",
      defaultRelay: !custom,
      serverKey: "nodekey:5d2e8b01c7a49f3e6d1b0a85c4e72f93",
      warnings: custom
        ? ["This server uses a custom relay (relay.lab.internal) instead of the public Tailcat relays. Only continue if you trust whoever sent you this invitation."]
        : [],
    };
  }

  function validLocalPort(p: number) {
    if (!Number.isInteger(p) || p < 0 || p > 65535) throw new Error("the local port must be between 1 and 65535 (or empty for automatic)");
  }

  function addFrom(p: {
    name: string;
    description: string;
    game: string;
    proto: string;
    port: number;
    localPort: number;
    relayHosts?: string[];
    defaultRelay?: boolean;
    serverFingerprint?: string | null;
  }): ServerView {
    const s = makeServer({
      id: `mock-new-${nextId++}`,
      name: p.name.trim(),
      description: p.description.trim(),
      game: p.game as Type,
      proto: p.proto as Protocol,
      port: p.port,
      localPort: p.localPort,
      relayHosts: p.relayHosts ?? PUBLIC_RELAY,
      defaultRelay: p.defaultRelay ?? true,
      serverFingerprint: p.serverFingerprint,
      addedAt: iso(),
      hasExecutable: !!settings.gameExecutables?.[p.game] || p.game === Type.MinecraftBedrock,
    });
    saved = [...saved, s];
    log(Kind.KindInfo, `Added server “${s.name}”`, s.id);
    emit("servers");
    return s;
  }

  function overviewNow() {
    const real = saved;
    const o = {
      servers: real.length,
      connected: 0,
      online: 0,
      publicKeyShort: MOCK_KEY.slice(0, 20) + "…",
      security: "Private keys stay on this device. Only devices approved by a server's owner can connect.",
      status: "idle",
    };
    for (const s of real) {
      const st = s.connection.state;
      if (st === "connected") o.connected++;
      else if ((st === "connecting" || st === "reconnecting") && o.status === "idle") o.status = "connecting";
      else if ((st === "error" || st === "unreachable") && o.connected === 0) o.status = "problem";
      if (st === "connected" ? s.connection.gameReachable === "yes" : s.check.state === "online") o.online++;
    }
    if (o.connected > 0) o.status = "connected";
    return o;
  }

  const api: MockBackend = {
    async version() {
      await wait();
      return { app: "GameLink", version: "0.1.0-alpha", transport: "Tailcat (WireGuard, direct with DERP relay fallback)" };
    },
    async identity() {
      await wait();
      return { publicKey: MOCK_KEY, created: iso(86_400_000 * 26), configDir: "C:\\Users\\player\\AppData\\Roaming\\GameLink", protection: "Windows DPAPI (tied to your Windows account)" };
    },
    async paths() {
      await wait();
      const d = "C:\\Users\\player\\AppData\\Roaming\\GameLink";
      return { configDir: d, configFile: `${d}\\servers.json`, identityFile: `${d}\\identity.json`, activityFile: `${d}\\activity.json`, logFile: `${d}\\gamelink.log` };
    },
    async overview() {
      await wait();
      return overviewNow();
    },
    async servers() {
      await wait();
      return clone(allServers());
    },
    async gameIcons() {
      return {};
    },
    async settings() {
      await wait();
      return clone(settings);
    },
    async checkRelease() {
      return null;
    },
    async openReleaseNotes() {},
    async activity(limit) {
      await wait();
      return clone(activity.slice(0, limit));
    },
    async clearActivity() {
      await wait();
      activity = [];
      emit("activity");
    },

    async join(id) {
      await wait();
      const s = find(id);
      if (!s) throw new Error("server not found");
      if (s.connection.state === "connecting" || s.connection.state === "connected") throw new Error("already connecting or connected");
      runJoin(s);
    },
    async disconnect(id) {
      await wait();
      const s = find(id);
      if (!s) return;
      const was = s.connection.state === "connected";
      const t = timers.get(id);
      if (t) clearTimeout(t);
      timers.delete(id);
      s.connection = idle();
      if (was) log(Kind.KindDisconnect, `Disconnected from “${s.name}”`, id);
      emit("servers");
    },
    async removeServer(id) {
      await wait();
      const s = saved.find((x) => x.id === id);
      if (!s) throw new Error("server not found");
      saved = saved.filter((x) => x.id !== id);
      log(Kind.KindInfo, `Removed server “${s.name}”`);
      emit("servers");
    },
    async launchGame(id): Promise<LaunchResult> {
      await wait();
      const s = find(id);
      if (!s) throw new Error("server not found");
      const c = s.connection;
      if ((c.state !== "connected" && c.state !== "reconnecting") || !c.localPort) throw new Error("join the server first");
      const address = `127.0.0.1:${c.localPort}`;
      const exe = settings.gameExecutables?.[s.game];
      if (!exe) {
        if (s.game !== Type.MinecraftBedrock) throw new Error("no executable is configured for this game (Settings → Local game executables)");
        launches.push({ id, target: expandLaunch(BEDROCK_URI, c.localPort) });
        log(Kind.KindInfo, `Opened ${s.gameName} and asked it to join ${address}`, id);
        return { autoConnect: true, address };
      }
      const tmpl = settings.gameLaunchArgs?.[s.game] ?? DEFAULT_LAUNCH_ARGS[s.game] ?? "";
      launches.push({ id, target: `${exe} ${expandLaunch(tmpl, c.localPort)}`.trim() });
      log(Kind.KindInfo, "Launched game", id);
      return { autoConnect: hasPlaceholder(tmpl), address };
    },
    async check(id) {
      await wait();
      const s = saved.find((x) => x.id === id);
      if (!s) return unchecked();
      s.check = { state: "checking", message: "Checking…", latencyMs: null, path: "", checkedAt: "" };
      emit("servers");
      schedule(`check-${id}`, Math.max(joinMs / 2, 0), () => {
        const knownReachable = id === "mock-survival" || id === "mock-factorio" || id.startsWith("mock-new-");
        const reachable = s.connection.state === "connected" ? s.connection.gameReachable : knownReachable ? "yes" : "unreachable";
        s.check = reachable === "yes"
          ? { state: "online", message: "Game server answered", latencyMs: id === "mock-factorio" ? 63 : 31, path: id === "mock-factorio" ? "relay" : "direct", checkedAt: iso() }
          : reachable === "no"
            ? { state: "offline", message: "Game server is not responding", latencyMs: id === "mock-factorio" ? 63 : 31, path: id === "mock-factorio" ? "relay" : "direct", checkedAt: iso() }
            : reachable === "unknown"
              ? { state: "unknown", message: "Agent online, game status unknown", latencyMs: null, path: "", checkedAt: iso() }
              : { state: "unreachable", message: `No response: the server may be offline, or this device is not approved for “${s.name}”`, latencyMs: null, path: "", checkedAt: iso() };
        emit("servers");
      });
      return clone(s.check);
    },
    async checkAll() {
      await wait();
      for (const s of saved) void api.check(s.id);
    },
    async diagnostics(id) {
      await wait();
      const s = saved.find((x) => x.id === id);
      if (!s) throw new Error("server not found");
      const c = s.connection;
      return [
        "GameLink 0.1.0-alpha diagnostics",
        `Server: ${s.name} (${s.gameName})`,
        `Protocol: ${s.proto}, tunnel port ${s.port}, requested local port ${s.localPort} (0 = automatic)`,
        `Relay: ${(s.relayHosts ?? []).join(", ")} (public relay: ${s.defaultRelay})`,
        `Server key: ${s.serverKey}`,
        `This device: ${MOCK_KEY}`,
        `State: ${c.state} - ${c.message}`,
        `Path: ${c.path}${c.latencyMs != null ? `, latency ${c.latencyMs.toFixed(1)} ms` : ""}`,
        `Game port reachable through tunnel: ${c.gameReachable}`,
        `Active connections: ${c.activeConns}, UDP flows: ${c.udpFlows}, reconnects: ${c.reconnects}, oversize UDP drops: ${c.oversizeDrops}`,
        `Last check: ${s.check.state} ${s.check.message}`,
        "Limits: UDP datagrams larger than 1232 bytes cannot be carried and are dropped.",
        "",
        "No secrets are included in this report.",
      ].join("\n");
    },

    async previewInvite(text) {
      await wait();
      return previewOf(text);
    },
    async importInvite(text, localPort) {
      await wait();
      const p = previewOf(text);
      validLocalPort(localPort);
      return clone(
        addFrom({ name: p.name, description: p.note, game: p.game, proto: p.proto, port: p.port, localPort, relayHosts: p.relayHosts ?? PUBLIC_RELAY, defaultRelay: p.defaultRelay }),
      );
    },
    async addServer(f: NewServerForm) {
      await wait();
      if (!f.name.trim()) throw new Error("give the server a name");
      if (f.address.trim().length < 12) throw new Error("that server address does not look right; copy it again from the owner's message");
      if (!Number.isInteger(f.port) || f.port < 1 || f.port > 65535) throw new Error("the tunnel port must be between 1 and 65535");
      validLocalPort(f.localPort);
      return clone(addFrom(f));
    },
    async updateServer(id, name, description, localPort) {
      await wait();
      validLocalPort(localPort);
      const s = saved.find((x) => x.id === id);
      if (!s) throw new Error("server not found");
      if (s.connection.state !== "disconnected" && s.localPort !== localPort) throw new Error("disconnect before changing the local port");
      if (!name.trim()) throw new Error("give the server a name");
      s.name = name.trim();
      s.description = description.trim();
      s.localPort = localPort;
      emit("servers");
      return clone(s);
    },
    async updateSettings(v) {
      await wait();
      const exe: Record<string, string> = {};
      for (const [k, p] of Object.entries(v.gameExecutables ?? {})) {
        const path = (p ?? "").trim();
        if (!path) continue;
        if (!/^([A-Za-z]:[\\/]|\/)/.test(path)) throw new Error(`${k}: use a full path to the game's executable`);
        exe[k] = path;
      }
      const args: Record<string, string> = { ...DEFAULT_LAUNCH_ARGS };
      for (const [k, a] of Object.entries(v.gameLaunchArgs ?? {})) {
        if (/[\r\n]/.test(a ?? "")) throw new Error("launch arguments must be a single line");
        args[k] = (a ?? "").trim();
      }
      settings = { ...v, gameExecutables: exe, gameLaunchArgs: args };
      for (const s of saved) s.hasExecutable = !!exe[s.game] || s.game === Type.MinecraftBedrock;
      emit("servers");
      emit("settings");
      return clone(settings);
    },

    async hostConnection(): Promise<HostConnection> {
      await wait();
      return { endpoint: hostEndpoint, configured: !!hostEndpoint };
    },
    async configureHost(endpoint, token): Promise<HostConnection> {
      await wait();
      if (!endpoint.trim()) throw new Error("enter a host endpoint");
      if (!token.trim() && !hostEndpoint) throw new Error("enter an administrator token for this host");
      hostEndpoint = endpoint.trim().replace(/\/$/, "");
      return { endpoint: hostEndpoint, configured: true };
    },
    async hostStatus(): Promise<HostSnapshot> {
      await wait();
      return clone(hostSnapshot);
    },
    async refreshHost(): Promise<HostSnapshot> {
      await wait();
      if (!hostEndpoint) throw new Error("configure a host connection first");
      hostSnapshot = {
        refreshedAt: iso(),
        games: [
          { id: "mock-host-minecraft", name: "Friends Minecraft", type: "minecraft-java", proto: Protocol.TCP, port: 25565, disabled: false, approvedDevices: 2, state: "running", detail: "Panel reported running", serverFingerprint: "c".repeat(64), canCopyInvitation: true, pending: [] },
          { id: "mock-host-factorio", name: "Weekend Factorio", type: "factorio", proto: Protocol.UDP, port: 34197, disabled: false, approvedDevices: 1, state: "unknown", detail: "Status not available", serverFingerprint: "d".repeat(64), canCopyInvitation: true, pending: [] },
        ],
      };
      return clone(hostSnapshot);
    },
    async importHostGame(gameID, serverFingerprint) {
      await wait();
      const game = hostSnapshot.games.find((g) => g.id === gameID && g.serverFingerprint === serverFingerprint);
      if (!game) throw new Error("refresh host games first");
      const gameType = game.type === "factorio" ? Type.Factorio : game.type === "minecraft-bedrock" ? Type.MinecraftBedrock : Type.MinecraftJava;
      return clone(addFrom({ name: game.name, description: "Added from your host", game: gameType, proto: GAME_INFO[gameType]?.proto ?? Protocol.TCP, port: GAME_INFO[gameType]?.port ?? 25565, localPort: 0, serverFingerprint: game.serverFingerprint }));
    },

    async copyLinkedInvitation(id, serverFingerprint) {
      await wait();
      const game = hostSnapshot.games.find((g) => g.id === id && g.serverFingerprint === serverFingerprint && g.canCopyInvitation);
      if (!game) throw new Error("The selected linked game is unavailable");
      await navigator.clipboard.writeText(`GameLink demo invitation for ${game.name} (example only)`);
    },
    async approveLinkedRequest(id, serverFingerprint, request) {
      await wait();
      const game = hostSnapshot.games.find((g) => g.id === id && g.serverFingerprint === serverFingerprint && !g.disabled);
      if (!game?.pending?.some((p) => p.id === request.id && p.publicKey === request.publicKey)) {
        throw new Error("That join request is no longer pending");
      }
      game.approvedDevices++;
      game.pending = game.pending.filter((p) => p.id !== request.id);
    },
    async dismissLinkedRequest(id, serverFingerprint, request) {
      await wait();
      const game = hostSnapshot.games.find((g) => g.id === id && g.serverFingerprint === serverFingerprint && !g.disabled);
      if (!game?.pending?.some((p) => p.id === request.id && p.publicKey === request.publicKey)) {
        throw new Error("That join request is no longer pending");
      }
      game.pending = game.pending.filter((p) => p.id !== request.id);
    },

    async hostedGames() {
      await wait();
      return clone(hosted);
    },
    async createHostedGame(form) {
      await wait();
      return createHosted(form, false);
    },
    async shareLanWorld(detected) {
      await wait();
      const existing = hosted.find((game) => game.lan && game.name === detected.name);
      if (!existing) return createHosted({ ...detected, game: Type.MinecraftJava }, true);
      existing.port = detected.port;
      existing.enabled = true;
      updateHosting(existing);
      return { id: existing.id };
    },
    async scanLanWorlds() {
      await wait();
      return seed ? [clone(world)] : [];
    },
    async stopLanScan() {
      await wait();
    },
    async approveHostedRequest(id, requestId, label) {
      await wait();
      const game = hostedGame(id);
      approveDevice(game, pendingRequest(game, requestId).publicKey, label);
    },
    async dismissHostedRequest(id, requestId) {
      await wait();
      const game = hostedGame(id);
      const request = pendingRequest(game, requestId);
      game.pending = game.pending.filter((entry) => entry !== request);
    },
    async approveHostedDevice(id, publicKey, label) {
      await wait();
      approveDevice(hostedGame(id), publicKey, label);
    },
    async revokeHostedDevice(id, publicKey) {
      await wait();
      const game = hostedGame(id);
      game.approved = game.approved.filter((device) => device.publicKey !== publicKey);
      updateHosting(game);
    },
    async setHostingEnabled(id, enabled) {
      await wait();
      const game = hostedGame(id);
      game.enabled = enabled;
      updateHosting(game);
    },
    async removeHostedGame(id) {
      await wait();
      hosted = hosted.filter((game) => game.id !== id);
    },
    async copyHostedInvitation(id) {
      await wait();
      hostedGame(id);
      await navigator.clipboard.writeText("GameLink demo invitation (example only)");
    },

    async copyText() {
      await wait();
    },
    async pickExecutable(title) {
      await wait();
      if (/factorio/i.test(title)) return "C:\\Games\\Factorio\\bin\\x64\\factorio.exe";
      if (/bedrock/i.test(title)) return "C:\\Program Files\\WindowsApps\\Minecraft.Bedrock\\Minecraft.Windows.exe";
      if (/java/i.test(title)) return "C:\\Games\\Minecraft\\Minecraft Launcher.exe";
      return "C:\\Games\\Custom\\game.exe";
    },
    async openConfigFolder() {
      await wait();
    },
    onUpdate(cb) {
      listeners.add(cb);
      return () => {
        listeners.delete(cb);
      };
    },

    emit,
    launches,
    patchServer(id, patch) {
      const s = find(id);
      if (s) Object.assign(s, patch);
      emit("servers");
    },
  };
  return api;
}
