// JSON views and inputs for the Rust desktop host. No runtime bindings.
/**
 * CheckView is the result of the most recent reachability check.
 */
export interface CheckView {
    /**
     * unknown, checking, online, offline, unreachable
     */
    "state": string;
    "message": string;
    "latencyMs": number | null;
    "path": string;
    "checkedAt": string;
}

/**
 * ConnectionView is the live connection state of a server.
 */
export interface ConnectionView {
    /**
     * disconnected, connecting, connected, reconnecting, unreachable, error
     */
    "state": string;
    "message": string;
    "localAddr": string;
    "localPort": number;
    "portFallback": boolean;

    /**
     * direct, relay, unknown
     */
    "path": string;
    "relay": string;
    "latencyMs": number | null;

    /**
     * yes, no, unknown
     */
    "gameReachable": string;
    "since": string;
    "activeConns": number;
    "udpFlows": number;
    "reconnects": number;
    "oversizeDrops": number;
    "bytesToServer": number;
    "bytesFromServer": number;

    /**
     * demo mode only
     */
    "simulated": boolean;
}

/**
 * IdentityView describes this device's identity.
 */
export interface IdentityView {
    "publicKey": string;
    "created": string;
    "configDir": string;
    "protection": string;
}

/**
 * InvitePreview describes an invitation before it is saved.
 */
export interface InvitePreview {
    "name": string;
    "game": GameId | "";
    "gameName": string;
    "proto": Proto | "";
    "port": number;
    "note": string;
    "relayHosts": string[] | null;
    "relayRegion": string;
    "defaultRelay": boolean;
    "serverKey": string;
    "warnings": string[] | null;
}

/**
 * LaunchResult says what starting a game did.
 */
export interface LaunchResult {
    /**
     * AutoConnect is true when the game was told to join the server itself.
     * When false the player chooses a saved entry or enters the address.
     */
    "autoConnect": boolean;
    "minecraftEntry"?: string;

    /**
     * Address is the local address the game connects to (127.0.0.1:port).
     */
    "address": string;
}

/**
 * NewServerForm is the manual "Add server" form.
 */
export interface NewServerForm {
    "name": string;
    "description": string;
    "game": string;
    "proto": string;
    "port": number;
    "localPort": number;
    "address": string;
}

/**
 * Overview feeds the dashboard summary cards.
 */
export interface Overview {
    "servers": number;
    "connected": number;
    "online": number;

    "publicKeyShort": string;

    /**
     * one-line honest summary
     */
    "security": string;

    /**
     * idle, connected, connecting, problem
     */
    "status": string;
}

/**
 * Paths lists where GameLink stores things.
 */
export interface Paths {
    "configDir": string;
    "configFile": string;
    "identityFile": string;
    "activityFile": string;
    "logFile": string;
}

/**
 * ServerView is one saved server as shown in the UI.
 */
export interface ServerView {
    "stablePort"?: number;
    "minecraftListed"?: boolean;
    "launchConnects"?: boolean;
    "id": string;
    "name": string;
    "serverFingerprint"?: string | null;
    "description": string;
    "game": GameId | "";
    "gameName": string;
    "proto": Proto | "";
    "port": number;
    "localPort": number;
    "defaultPort": number;
    "addedAt": string;
    "relayHosts": string[] | null;
    "defaultRelay": boolean;
    "serverKey": string;
    "hint": string;
    "hasExecutable": boolean;
    "demo": boolean;
    "connection": ConnectionView;
    "check": CheckView;
}

/**
 * SettingsView is the editable preferences.
 */
export interface SettingsView {
    "demoMode": boolean;
    "autoReconnect": boolean;
    "checkOnStartup": boolean;
    "checkForUpdates": boolean;
    /** Read-only: false when this build has no release verification key. */
    "canCheckForUpdates": boolean;
    "verboseLogging": boolean;
    "allowLocalRelay": boolean;
    "minecraftServerList": boolean;
    "prismInstancesDir": string;
    "presenceNotifications": boolean;
    "minecraftSyncError": string;
    "gameExecutables": { [_ in string]?: string } | null;

    /**
     * GameLaunchArgs is the launch template in effect for every game type,
     * with {host}, {port} and {address} placeholders.
     */
    "gameLaunchArgs": { [_ in string]?: string } | null;
}

/**
 * Version describes the build.
 */
export interface Version {
    "app": string;
    "version": string;
    "transport": string;
}

export interface ReleaseNotice {
  version: string;
  severity: "normal" | "security";
  releaseNotesUrl: string;
  summary: string | null;
  canInstall: boolean;
  installerError: string | null;
  linkedAgent: LinkedAgentNotice | null;
}

export type LinkedAgentNotice =
  | { kind: "available"; nodeName: string; offerId: string }
  | { kind: "manual" }
  | { kind: "unreachable" };

export type AgentUpdateProgress = "preparing" | "installing" | "restarting" | "checkingHealth";

export type UpdateProgress =
  | { stage: "downloading"; downloaded: number; total: number | null }
  | { stage: "verifying" }
  | { stage: "installing" };

/**
 * Event is one entry of the activity history.
 */
export interface ActivityEvent {
    "time": string;
    "kind": "" | "info" | "connect" | "disconnect" | "warning" | "error" | "security";
    "serverId"?: string;
    "message": string;
}


export interface HostConnection {
  endpoint: string;
  configured: boolean;
}

export interface HostGame {
  id: string;
  name: string;
  type: string;
  disabled: boolean;
  approvedDevices: number;
  proto?: Proto;
  port?: number;
  serverFingerprint?: string | null;
  legacyAgent?: boolean;
  canCopyInvitation?: boolean;
  pending?: PendingDevice[];
  state: string;
  detail: string;
}

export interface HostSnapshot {
  games: HostGame[];
  refreshedAt: string;
  refreshing?: boolean;
}

export type DesktopGameType = Exclude<GameId, "custom">;

export interface HostedGameForm {
  name: string;
  game: DesktopGameType;
  port: number;
}

export interface LanWorld {
  name: string;
  port: number;
}

export interface HostedDevice {
  publicKey: string;
  label: string;
  lastConnected: string | null;
}

export interface PendingDevice {
  id: number;
  publicKey: string;
  lastSeen: string;
}

export interface HostedGame extends HostedGameForm {
  id: string;
  serverFingerprint?: string | null;
  lan: boolean;
  enabled: boolean;
  /** A shared LAN world stopped announcing; the tunnel waits for it on the same port. */
  paused?: boolean;
  state: "inactive" | "serving" | "error";
  detail: string;
  approved: HostedDevice[];
  pending: PendingDevice[];
}

export type Screen = "dashboard" | "servers" | "people" | "host" | "host-admin" | "settings";

export type UpdateTopic = "servers" | "activity" | "settings";

/** Game ids as the Go side spells them (games.Type). */
export type GameId = "minecraft-java" | "minecraft-bedrock" | "factorio" | "custom";
export type Proto = "tcp" | "udp";

/** Wire constants shared by browser fixtures and the mock backend. */
export enum Type {
  $zero = "",
  MinecraftJava = "minecraft-java",
  MinecraftBedrock = "minecraft-bedrock",
  Factorio = "factorio",
  Custom = "custom",
}
export enum Protocol { $zero = "", TCP = "tcp", UDP = "udp" }
export enum Kind {
  $zero = "", KindInfo = "info", KindConnect = "connect",
  KindDisconnect = "disconnect", KindWarning = "warning",
  KindError = "error", KindSecurity = "security",
}
