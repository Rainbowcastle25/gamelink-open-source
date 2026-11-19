import type { Backend } from "./backend";
import type { UpdateProgress, UpdateTopic } from "./types";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const call = <T>(method: string, args: Record<string, unknown> = {}): Promise<T> =>
  invoke<T>("desktop", { request: { ...args, method } });

// Preserve scan/close ordering when the Host view unmounts or remounts.
let discovery: Promise<unknown> = Promise.resolve();
function lanCall<T>(method: string): Promise<T> {
  const next = discovery.then(() => call<T>(method));
  discovery = next.catch(() => {});
  return next;
}

export const realBackend: Backend = {
  version: async () => call("version"),
  identity: async () => call("identity"),
  paths: async () => call("paths"),
  overview: async () => call("overview"),
  servers: async () => (await call<Awaited<ReturnType<Backend["servers"]>>>("servers")) ?? [],
  settings: async () => call("settings"),
  gameIcons: async () => (await call<Record<string, string> | null>("gameIcons")) ?? {},
  checkRelease: async () => invoke("check_release"),
  installUpdate: async (version, onProgress) => {
    const progress = new Channel<UpdateProgress>(onProgress);
    await invoke("install_update", { version, progress });
  },
  // Windows hands off to the NSIS installer, which relaunches after installation.
  relaunch: async () => invoke("plugin:process|restart"),
  openReleaseNotes: async () => call("openReleaseNotes"),
  activity: async (limit) => (await call<Awaited<ReturnType<Backend["activity"]>>>("activity", { limit })) ?? [],
  clearActivity: async () => call("clearActivity"),

  join: async (id) => call("join", { id }),
  disconnect: async (id) => call("disconnect", { id }),
  removeServer: async (id) => call("removeServer", { id }),
  launchGame: async (id) => call("launchGame", { id }),
  check: async (id) => call("check", { id }),
  checkAll: async () => call("checkAll"),
  diagnostics: async (id) => call("diagnostics", { id }),

  previewInvite: async (text) => call("previewInvite", { text }),
  importInvite: async (text, localPort) => call("importInvite", { text, localPort }),
  addServer: async (form) => call("addServer", { form }),
  updateServer: async (id, name, description, localPort) => call("updateServer", { id, name, description, localPort }),
  updateSettings: async (v) => call("updateSettings", { settings: v }),

  hostConnection: async () => call("hostConnection"),
  configureHost: async (endpoint, token) => call("configureHost", { endpoint, token }),
  hostStatus: async () => call("hostStatus"),
  refreshHost: async () => call("refreshHost"),
  importHostGame: async (gameID, serverFingerprint) => call("importHostGame", { gameID, serverFingerprint }),
  copyLinkedInvitation: async (id, serverFingerprint) => call("copyLinkedInvitation", { id, serverFingerprint }),
  approveLinkedRequest: async (id, serverFingerprint, request, label) =>
    call("approveLinkedRequest", { id, serverFingerprint, requestId: request.id, publicKey: request.publicKey, label }),
  dismissLinkedRequest: async (id, serverFingerprint, request) =>
    call("dismissLinkedRequest", { id, serverFingerprint, requestId: request.id, publicKey: request.publicKey }),

  hostedGames: async () => call("hostedGames"),
  createHostedGame: async (form) => call("createHostedGame", { ...form }),
  shareLanWorld: async (world) => call("shareLanWorld", { ...world }),
  scanLanWorlds: async () => lanCall("scanLanWorlds"),
  stopLanScan: async () => lanCall("stopLanScan"),
  approveHostedRequest: async (id, requestId, label) => call("approveHostedRequest", { id, requestId, label }),
  dismissHostedRequest: async (id, requestId) => call("dismissHostedRequest", { id, requestId }),
  approveHostedDevice: async (id, publicKey, label) => call("approveHostedDevice", { id, publicKey, label }),
  revokeHostedDevice: async (id, publicKey) => call("revokeHostedDevice", { id, publicKey }),
  setHostingEnabled: async (id, enabled) => call("setHostingEnabled", { id, enabled }),
  removeHostedGame: async (id) => call("removeHostedGame", { id }),
  copyHostedInvitation: async (id) => call("copyHostedInvitation", { id }),

  copyText: async (text) => call("copyText", { text }),
  pickExecutable: async (title) => call("pickExecutable", { title }),
  openConfigFolder: async () => call("openConfigFolder"),

  onUpdate(cb) {
    let cancelled = false;
    let off: (() => void) | undefined;
    void listen<UpdateTopic>("gamelink:update", (ev) => cb(ev.payload)).then((unlisten) => {
      if (cancelled) unlisten();
      else off = unlisten;
    });
    return () => { cancelled = true; off?.(); };
  },
};
