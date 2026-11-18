import type { Backend } from "./backend";
import type { UpdateTopic } from "./types";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const call = <T>(method: string, args: Record<string, unknown> = {}): Promise<T> =>
  invoke<T>("desktop", { request: { ...args, method } });

export const realBackend: Backend = {
  version: async () => call("version"),
  identity: async () => call("identity"),
  paths: async () => call("paths"),
  overview: async () => call("overview"),
  servers: async () => (await call<Awaited<ReturnType<Backend["servers"]>>>("servers")) ?? [],
  settings: async () => call("settings"),
  checkRelease: async () => call("checkRelease"),
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
  importHostGame: async (gameID) => call("importHostGame", { gameID }),

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
