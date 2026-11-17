import { afterEach, describe, expect, it, vi } from "vitest";
import { conn } from "../test/utils";
import { createMockBackend } from "./mockBackend";
import { GameLinkStore } from "./store";

function setup() {
  const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
  const copy = vi.spyOn(backend, "copyText");
  const store = new GameLinkStore(backend);
  const stop = store.start();
  return { backend, store, copy, stop };
}

const settle = () => vi.waitFor(() => new Promise((r) => setTimeout(r, 5)));

describe("Play (one-click join and launch)", () => {
  let stop: (() => void) | undefined;
  afterEach(() => stop?.());

  it("joins, then starts the game once connected, and copies the address when the game cannot be told", async () => {
    const t = setup();
    stop = t.stop;
    await t.store.loadAll();
    await t.store.play("mock-survival");
    await vi.waitFor(() => expect(t.backend.launches).toHaveLength(1));
    // Minecraft's own launcher takes no server argument: started plainly, address copied.
    expect(t.backend.launches[0]).toEqual({ id: "mock-survival", target: "C:\\Games\\Minecraft\\Minecraft Launcher.exe" });
    expect(t.copy).toHaveBeenCalledWith("127.0.0.1:25565");
  });

  it("waits through a failed attempt, launches after Retry connects, and never relaunches on reconnect", async () => {
    const t = setup();
    stop = t.stop;
    await t.store.loadAll();
    // The sample Bedrock server never answers.
    await t.store.play("mock-bedrock");
    await vi.waitFor(() => expect(t.store.getState().servers.find((s) => s.id === "mock-bedrock")?.connection.state).toBe("unreachable"));
    expect(t.backend.launches).toHaveLength(0);
    expect(t.store.isPlayPending("mock-bedrock")).toBe(true);

    // Retry connects this time.
    t.backend.patchServer("mock-bedrock", { connection: conn({ state: "connected", localAddr: "127.0.0.1:19133", localPort: 19133 }) });
    await vi.waitFor(() => expect(t.backend.launches).toHaveLength(1));
    expect(t.backend.launches[0].target).toBe("minecraft://connect?serverUrl=127.0.0.1&serverPort=19133");
    expect(t.copy).not.toHaveBeenCalled();

    t.backend.patchServer("mock-bedrock", { connection: conn({ state: "reconnecting", localPort: 19133 }) });
    t.backend.patchServer("mock-bedrock", { connection: conn({ state: "connected", localPort: 19133 }) });
    await settle();
    expect(t.backend.launches).toHaveLength(1);
  });

  it("Disconnect cancels a pending Play", async () => {
    const t = setup();
    stop = t.stop;
    await t.store.loadAll();
    await t.store.play("mock-bedrock");
    await t.store.disconnect("mock-bedrock");
    expect(t.store.isPlayPending("mock-bedrock")).toBe(false);
    t.backend.patchServer("mock-bedrock", { connection: conn({ state: "connected", localPort: 19132 }) });
    await settle();
    expect(t.backend.launches).toHaveLength(0);
  });

  it("plain Join never starts the game", async () => {
    const t = setup();
    stop = t.stop;
    await t.store.loadAll();
    await t.store.join("mock-survival");
    await vi.waitFor(() => expect(t.store.getState().servers.find((s) => s.id === "mock-survival")?.connection.state).toBe("connected"));
    await settle();
    expect(t.backend.launches).toHaveLength(0);
  });
});
