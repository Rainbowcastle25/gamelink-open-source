import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createMockBackend } from "../lib/mockBackend";
import { conn, renderApp, sv } from "../test/utils";
import { ConnectionPanel } from "./ConnectionPanel";
import { ServerCard } from "./ServerCard";
import { ServerList } from "./ServerList";
import { StatusIndicator } from "./StatusIndicator";

const noop = () => {};
const handlers = { onJoin: noop, onDisconnect: noop, onCheck: noop, onLaunch: noop, onOpenRequests: noop };

describe("StatusIndicator", () => {
  it.each([
    ["idle", "Ready"],
    ["connecting", "Connecting"],
    ["connected", "Connected"],
    ["problem", "Problem"],
  ])("shows %s as %s", (status, label) => {
    render(<StatusIndicator status={status} />);
    expect(screen.getByRole("status")).toHaveTextContent(label);
  });

  it("falls back to Ready for unknown values instead of showing a green dot", () => {
    render(<StatusIndicator status="online" />);
    expect(screen.getByRole("status")).toHaveTextContent("Ready");
  });
});

describe("ServerList", () => {
  it("shows the installed game icon and falls back if it cannot be displayed", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const src = "data:image/png;base64,AA==";
    let finishIcons!: (icons: Record<string, string>) => void;
    vi.spyOn(backend, "gameIcons").mockReturnValue(new Promise((resolve) => { finishIcons = resolve; }));
    vi.spyOn(backend, "servers").mockResolvedValue([sv({ game: "factorio" })]);
    renderApp(<ServerList />, { backend });

    const card = await screen.findByRole("article", { name: "Test Server" });
    const tile = within(card).getByRole("img", { name: "Factorio" });
    expect(tile.querySelector("svg")).toBeInTheDocument();
    finishIcons({ factorio: src });
    await waitFor(() => expect(tile.querySelector("img")).toHaveAttribute("src", src));
    fireEvent.error(tile.querySelector("img")!);
    expect(tile.querySelector("img")).toBeNull();
    expect(tile.querySelector("svg")).toBeInTheDocument();
  });
});

describe("ServerCard", () => {
  it("disconnected and not checked: shows Not checked, a Check action and an enabled Join", async () => {
    const onJoin = vi.fn();
    const onCheck = vi.fn();
    const { user } = renderApp(<ServerCard server={sv()} {...handlers} onJoin={onJoin} onCheck={onCheck} />);
    expect(screen.getByText("Not checked")).toBeInTheDocument();
    const join = screen.getByRole("button", { name: "Join" });
    expect(join).toBeEnabled();
    await user.click(join);
    expect(onJoin).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Check" }));
    expect(onCheck).toHaveBeenCalledTimes(1);
  });

  it("a launchable game gets Play (join and start) plus Join only; demo servers keep plain Join", async () => {
    const onJoin = vi.fn();
    const onPlay = vi.fn();
    const { user, unmount } = renderApp(<ServerCard server={sv({ hasExecutable: true })} {...handlers} onJoin={onJoin} onPlay={onPlay} />);
    await user.click(screen.getByRole("button", { name: "Play" }));
    expect(onPlay).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole("button", { name: "Join only" }));
    expect(onJoin).toHaveBeenCalledTimes(1);
    unmount();

    renderApp(<ServerCard server={sv({ hasExecutable: true, demo: true })} {...handlers} onPlay={onPlay} />);
    expect(screen.queryByRole("button", { name: "Play" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Join" })).toBeInTheDocument();
  });

  it("online with a measured latency shows the milliseconds", () => {
    renderApp(<ServerCard server={sv({ check: { state: "online", latencyMs: 42.4 } })} {...handlers} />);
    expect(screen.getByText("Online")).toBeInTheDocument();
    expect(screen.getByText("42 ms")).toBeInTheDocument();
  });

  it("online without a latency does not invent one", () => {
    renderApp(<ServerCard server={sv({ check: { state: "online", latencyMs: null } })} {...handlers} />);
    expect(screen.getByText("Online")).toBeInTheDocument();
    expect(screen.queryByText(/\bms\b/)).not.toBeInTheDocument();
  });

  it("checking shows Checking...", () => {
    renderApp(<ServerCard server={sv({ check: { state: "checking" } })} {...handlers} />);
    expect(screen.getByText("Checking...")).toBeInTheDocument();
  });

  it("connecting disables the primary button", () => {
    renderApp(<ServerCard server={sv({ connection: { state: "connecting" } })} {...handlers} />);
    expect(screen.getByRole("button", { name: /connecting/i })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Join" })).not.toBeInTheDocument();
  });

  it("a Join in flight shows Connecting... before the backend reports it", () => {
    renderApp(<ServerCard server={sv()} busy="joining" {...handlers} />);
    expect(screen.getByRole("button", { name: /connecting/i })).toBeDisabled();
  });

  it("connected shows the local address, Copy Address and Disconnect", async () => {
    const onDisconnect = vi.fn();
    const { user } = renderApp(
      <ServerCard
        server={sv({ connection: { state: "connected", localAddr: "127.0.0.1:25565", localPort: 25565, path: "direct", latencyMs: 34 } })}
        {...handlers}
        onDisconnect={onDisconnect}
      />,
    );
    expect(screen.getByText("127.0.0.1:25565")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy Address" })).toBeInTheDocument();
    expect(screen.getByText("Direct connection")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(onDisconnect).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["yes", "Connected"],
    ["no", "Game server offline"],
    ["unknown", "Agent online, game status unknown"],
  ])("live reachability %s is shown as %s", (gameReachable, label) => {
    const { rerender } = renderApp(<ServerCard server={sv({ connection: { state: "connected", gameReachable } })} {...handlers} />);
    expect(screen.getByText(label)).toBeInTheDocument();
    // Status callbacks replace the live reachability fact while the tunnel stays connected.
    rerender(<ServerCard server={sv({ connection: { state: "connected", gameReachable: "yes" } })} {...handlers} />);
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("copies the local address to the clipboard and confirms", async () => {
    const { user } = renderApp(<ServerCard server={sv({ connection: { state: "connected", localAddr: "127.0.0.1:25565", localPort: 25565 } })} {...handlers} />);
    const write = vi.spyOn(navigator.clipboard, "writeText");
    await user.click(screen.getByRole("button", { name: "Copy Address" }));
    expect(write).toHaveBeenCalledWith("127.0.0.1:25565");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
  });

  it("unreachable explains the likely causes and offers Retry and the Requests screen", async () => {
    const onJoin = vi.fn();
    const onOpenRequests = vi.fn();
    const { user } = renderApp(
      <ServerCard server={sv({ connection: { state: "unreachable", message: "No response" } })} {...handlers} onJoin={onJoin} onOpenRequests={onOpenRequests} />,
    );
    expect(screen.getByText(/may be offline, or this device has not been approved/i)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(onJoin).toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Open Requests" }));
    expect(onOpenRequests).toHaveBeenCalled();
  });

  it("error shows the backend message", () => {
    renderApp(<ServerCard server={sv({ connection: { state: "error", message: "Relay handshake failed" } })} {...handlers} />);
    expect(screen.getByText("Relay handshake failed")).toBeInTheDocument();
  });

  it("an unreachable check says No response and explains why in a tooltip", () => {
    renderApp(<ServerCard server={sv({ check: { state: "unreachable" } })} {...handlers} />);
    expect(screen.getByText("No response")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /cannot tell an offline server apart/i })).toBeInTheDocument();
  });

  it("demo servers carry a Demo tag and no Check action", () => {
    renderApp(<ServerCard server={sv({ demo: true, name: "Demo: Survival World", check: { message: "Demo servers are not real" } })} {...handlers} />);
    expect(screen.getByText("Demo")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Check" })).not.toBeInTheDocument();
  });

  it("never renders a player count", () => {
    const states = [
      sv({ check: { state: "online", latencyMs: 20 } }),
      sv({ connection: { state: "connected", localAddr: "127.0.0.1:25565", localPort: 25565, latencyMs: 20, path: "direct" } }),
      sv({ connection: { state: "unreachable" } }),
      sv({ demo: true }),
    ];
    for (const s of states) {
      const { container, unmount } = renderApp(<ServerCard server={s} {...handlers} />);
      expect(container.textContent ?? "").not.toMatch(/\bplayers?\b/i);
      unmount();
    }
  });

  it("manage mode exposes Edit, Diagnostics, Remove behind the details toggle", async () => {
    const onRemove = vi.fn();
    const { user } = renderApp(<ServerCard server={sv()} {...handlers} manage={{ onEdit: noop, onDiagnostics: noop, onRemove }} />);
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /show details/i }));
    expect(screen.getByText(/tunnel port 25565/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Remove" }));
    expect(onRemove).toHaveBeenCalled();
  });
});

describe("ConnectionPanel", () => {
  const connected = (over = {}) => sv({ connection: { state: "connected", localAddr: "127.0.0.1:34197", localPort: 34197, path: "relay", ...over } });

  it("warns when the game server is not accepting connections", () => {
    renderApp(<ConnectionPanel server={connected({ gameReachable: "no" })} />);
    expect(screen.getByText("Connected to the server, but the game server is not accepting connections - is it running?")).toBeInTheDocument();
  });

  it("warns about dropped oversized UDP packets and states the limit", () => {
    renderApp(<ConnectionPanel server={connected({ oversizeDrops: 3 })} />);
    const note = screen.getByText(/3 oversized UDP packets were dropped/);
    expect(note).toHaveTextContent("1232 bytes");
  });

  it("explains a port fallback", () => {
    renderApp(<ConnectionPanel server={{ ...connected({ portFallback: true, localPort: 34198, localAddr: "127.0.0.1:34198" }), stablePort: 25565 }} />);
    expect(screen.getByText(/Stable port 25565 was unavailable. Using temporary port 34198/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Copy Address" })).toBeVisible();
  });

  it("keeps the synced Minecraft entry during a port fallback", () => {
    const server = { ...connected({ portFallback: true, localPort: 34198, localAddr: "127.0.0.1:34198" }), stablePort: 25565, minecraftListed: true };
    renderApp(<ConnectionPanel server={server} />);
    expect(screen.getByText(/Using temporary port 34198/)).not.toHaveTextContent("still points to the stable port");
    expect(screen.getByText(`In Minecraft Multiplayer, select “${server.name} (GameLink)”.`)).toBeVisible();
  });

  it("guides Minecraft players to the saved entry with the address collapsed", () => {
    const server = { ...connected(), minecraftListed: true };
    renderApp(<ConnectionPanel server={server} />);
    expect(screen.getByText(`In Minecraft Multiplayer, select “${server.name} (GameLink)”.`)).toBeVisible();
    expect(screen.getByText("Show local address").closest("details")).not.toHaveAttribute("open");
  });

  it("shows path and latency only when known", () => {
    const { unmount } = renderApp(<ConnectionPanel server={connected({ path: "unknown", latencyMs: null })} />);
    expect(screen.getByText("Path not measured")).toBeInTheDocument();
    expect(screen.queryByText(/\bms\b/)).not.toBeInTheDocument();
    unmount();
    renderApp(<ConnectionPanel server={connected({ path: "relay", latencyMs: 71 })} />);
    expect(screen.getByText("Relayed")).toBeInTheDocument();
    expect(screen.getByText("71 ms")).toBeInTheDocument();
  });

  it("shows no warnings on a healthy connection", () => {
    renderApp(<ConnectionPanel server={connected({ gameReachable: "yes" })} />);
    expect(screen.queryByText(/not accepting connections/)).not.toBeInTheDocument();
    expect(screen.queryByText(/oversized/)).not.toBeInTheDocument();
  });

  it("explains when the game endpoint has not been checked", () => {
    renderApp(<ConnectionPanel server={connected({ gameReachable: "unknown" })} />);
    expect(screen.getByText("GameLink is connected, but the game server status is unknown. The game endpoint has not responded to a check yet.")).toBeInTheDocument();
  });

  it("Launch Game only appears when an executable is configured", async () => {
    const onLaunch = vi.fn();
    const { unmount } = renderApp(<ConnectionPanel server={connected()} onLaunch={onLaunch} />);
    expect(screen.queryByRole("button", { name: "Launch Game" })).not.toBeInTheDocument();
    unmount();
    const r = renderApp(<ConnectionPanel server={{ ...connected(), hasExecutable: true }} onLaunch={onLaunch} />);
    await r.user.click(screen.getByRole("button", { name: "Launch Game" }));
    expect(onLaunch).toHaveBeenCalled();
  });

  it("reconnecting shows a calm Reconnecting... message", () => {
    renderApp(<ConnectionPanel server={sv({ connection: conn({ state: "reconnecting", localAddr: "127.0.0.1:25565" }) })} />);
    const status = screen.getByRole("status");
    expect(within(status).getByText(/Reconnecting\.\.\./)).toBeInTheDocument();
  });
});
