import { render } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ReactElement } from "react";
import { Protocol, Type } from "../lib/types";
import { createMockBackend, type MockBackend } from "../lib/mockBackend";
import { NavProvider } from "../lib/nav";
import { GameLinkProvider } from "../lib/store";
import type { CheckView, ConnectionView, ServerView } from "../lib/types";

export function conn(over: Partial<ConnectionView> = {}): ConnectionView {
  return {
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
    ...over,
  };
}

export function check(over: Partial<CheckView> = {}): CheckView {
  return { state: "unknown", message: "Not checked yet", latencyMs: null, path: "", checkedAt: "", ...over };
}

type ServerOverrides = Omit<Partial<ServerView>, "connection" | "check"> & { connection?: Partial<ConnectionView>; check?: Partial<CheckView> };

/** A plain ServerView for presentational tests. */
export function sv(over: ServerOverrides = {}): ServerView {
  const { connection, check: chk, ...rest } = over;
  return {
    id: "srv-1",
    name: "Test Server",
    description: "",
    game: Type.MinecraftJava,
    gameName: "Minecraft: Java Edition",
    proto: Protocol.TCP,
    port: 25565,
    localPort: 0,
    defaultPort: 25565,
    addedAt: new Date().toISOString(),
    relayHosts: ["relay.tailcat.example"],
    defaultRelay: true,
    serverKey: "nodekey:b7d1f40c9a2e5683d0c4a1f79e28b563",
    hint: "In Minecraft choose Multiplayer, then Add Server.",
    hasExecutable: false,
    demo: false,
    ...rest,
    connection: conn(connection),
    check: check(chk),
  };
}

/** Renders inside the provider with a zero-latency mock backend. */
export function renderApp(ui: ReactElement, opts: { backend?: MockBackend; demoMode?: boolean } = {}) {
  // Must run before render: user-event installs its own clipboard stub.
  const user = userEvent.setup();
  const backend = opts.backend ?? createMockBackend({ delayMs: 0, joinMs: 0, demoMode: opts.demoMode });
  const utils = render(
    <GameLinkProvider backend={backend}>
      <NavProvider>{ui}</NavProvider>
    </GameLinkProvider>,
  );
  return { backend, user, ...utils };
}
