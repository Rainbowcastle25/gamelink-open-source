import { beforeEach, describe, expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
import { realBackend } from "./realBackend";

describe("Tauri transport", () => {
  beforeEach(() => vi.resetAllMocks());
  it("preserves named arguments and backend errors", async () => {
    mocks.invoke.mockResolvedValue({ id: "saved" });
    await realBackend.updateServer("server", "name", "description", 1234);
    expect(mocks.invoke).toHaveBeenCalledWith("desktop", { request: {
      method: "updateServer", id: "server", name: "name", description: "description", localPort: 1234,
    } });
    mocks.invoke.mockRejectedValue("join the server first");
    await expect(realBackend.launchGame("server")).rejects.toBe("join the server first");
  });
  it("normalizes empty Go lists", async () => {
    mocks.invoke.mockResolvedValue(null);
    expect(await realBackend.servers()).toEqual([]);
    expect(await realBackend.activity(20)).toEqual([]);
  });
  it("releases a subscription cancelled before native registration resolves", async () => {
    let resolve!: (off: () => void) => void;
    const off = vi.fn();
    mocks.listen.mockReturnValue(new Promise((done) => { resolve = done; }));
    const cancel = realBackend.onUpdate(vi.fn());
    cancel();
    resolve(off);
    await Promise.resolve();
    expect(off).toHaveBeenCalledOnce();
  });
  it("forwards native topic payloads and unsubscribes", async () => {
    const off = vi.fn();
    const cb = vi.fn();
    mocks.listen.mockResolvedValue(off);
    const cancel = realBackend.onUpdate(cb);
    await Promise.resolve();
    mocks.listen.mock.calls[0][1]({ payload: "servers" });
    expect(cb).toHaveBeenCalledWith("servers");
    cancel();
    expect(off).toHaveBeenCalledOnce();
  });
});
