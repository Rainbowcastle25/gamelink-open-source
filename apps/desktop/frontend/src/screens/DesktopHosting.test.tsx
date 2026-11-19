import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { Toaster } from "../components/Toaster";
import { Type } from "../lib/types";
import { createMockBackend } from "../lib/mockBackend";
import { renderApp } from "../test/utils";
import { DashboardScreen } from "./DashboardScreen";
import { ServersScreen } from "./ServersScreen";
import { LocalHostScreen } from "./LocalHostScreen";
import { PeopleScreen } from "./PeopleScreen";

describe("Host on this PC", () => {
  it("shares a detected world, offers native copy and stops discovery on leaving", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const share = vi.spyOn(backend, "shareLanWorld");
    const copy = vi.spyOn(backend, "copyHostedInvitation");
    const stop = vi.spyOn(backend, "stopLanScan");
    const { user, unmount, container } = renderApp(<LocalHostScreen />, { backend });

    expect(await screen.findByText("Share “Demo: LAN Cabin”?")).toBeInTheDocument();
    expect(share).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Share world" }));
    await screen.findByRole("heading", { name: "Demo: LAN Cabin" });
    expect(share).toHaveBeenCalledWith({ name: "Demo: LAN Cabin", port: 51734 });
    await user.click(screen.getByRole("button", { name: "Copy invitation" }));
    await waitFor(() => expect(copy).toHaveBeenCalledWith("demo-hosted-1"));
    expect(await screen.findByText("Invitation copied. Share it privately.")).toBeInTheDocument();
    expect(container).not.toHaveTextContent("GameLink demo invitation (example only)");
    unmount();
    expect(stop).toHaveBeenCalledOnce();
  });

  it("offers the same hosted game for a reopened world on a different port", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const before = await backend.shareLanWorld({ name: "Demo: LAN Cabin", port: 51000 });
    const { user } = renderApp(<LocalHostScreen />, { backend });
    await user.click(await screen.findByRole("button", { name: "Update LAN port" }));
    await waitFor(async () => {
      const hosted = (await backend.hostedGames()).filter((game) => game.lan);
      expect(hosted).toMatchObject([{ id: before.id, port: 51734 }]);
    });
  });

  it("shows a closed shared LAN world as paused on Host and People", async () => {
    const backend = createMockBackend({ delayMs: 0, seed: false });
    const detail = "World closed or not announced; sharing resumes if it reopens on the same port";
    vi.spyOn(backend, "hostedGames").mockResolvedValue([{
      id: "demo-hosted-lan", game: Type.MinecraftJava, name: "Demo: Closed Cabin", port: 51000,
      lan: true, enabled: true, paused: true, state: "inactive", detail, approved: [], pending: [],
    }]);
    const host = renderApp(<LocalHostScreen />, { backend });
    const card = (await screen.findByRole("heading", { name: "Demo: Closed Cabin" })).closest("li")!;
    expect(card).toHaveTextContent("Sharing paused");
    expect(card).toHaveTextContent(detail);
    expect(within(card).getByRole("button", { name: "Stop sharing" })).toBeInTheDocument();
    host.unmount();
    renderApp(<PeopleScreen />, { backend });
    expect(await screen.findByRole("article", { name: "Demo: Closed Cabin" })).toHaveTextContent("Sharing paused");
  });

  it("creates a manual server when multicast is unavailable", async () => {
    const backend = createMockBackend({ delayMs: 0, seed: false });
    vi.spyOn(backend, "scanLanWorlds").mockRejectedValue("Could not join multicast");
    const create = vi.spyOn(backend, "createHostedGame");
    const { user } = renderApp(<LocalHostScreen />, { backend });

    expect(await screen.findByRole("alert")).toHaveTextContent("You can enter the port below.");
    await user.type(screen.getByLabelText("Server name"), "Weekend Bedrock");
    await user.selectOptions(screen.getByLabelText("Game"), "minecraft-bedrock");
    expect(screen.getByLabelText("Local port")).toHaveValue(19132);
    await user.clear(screen.getByLabelText("Local port"));
    await user.type(screen.getByLabelText("Local port"), "19000");
    await user.click(screen.getByRole("button", { name: "Host server" }));
    expect(await screen.findByRole("heading", { name: "Weekend Bedrock" })).toBeInTheDocument();
    expect(create).toHaveBeenCalledWith({ name: "Weekend Bedrock", game: "minecraft-bedrock", port: 19000 });
  });
});

describe("Hosted game access", () => {
  it("shows labels, shortened keys and last connections, then revokes only after confirmation", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const revoke = vi.spyOn(backend, "revokeHostedDevice");
    const { user } = renderApp(<PeopleScreen />, { backend });
    const game = await screen.findByRole("article", { name: "Demo: Factory" });
    expect(game).toHaveTextContent("Demo: Alex · aaaaaaaaaaaa…");
    expect(game).toHaveTextContent("Last connected");
    expect(game.querySelector("time")).toHaveAttribute("datetime");
    expect(game).not.toHaveTextContent("a".repeat(64));
    await user.click(within(game).getByRole("button", { name: "Revoke" }));
    expect(revoke).not.toHaveBeenCalled();
    await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Revoke device" }));
    expect(await within(game).findByText("No approved devices.")).toBeInTheDocument();
    expect(revoke).toHaveBeenCalledWith("demo-hosted-factory", "nodekey:" + "a".repeat(64));
  });

  it("requires explicit approval after reviewing a join request", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const approve = vi.spyOn(backend, "approveHostedRequest");
    const { user } = renderApp(<PeopleScreen />, { backend });
    await user.click(await screen.findByRole("button", { name: "Review request" }));
    const dialog = screen.getByRole("dialog", { name: "Approve a device for Demo: Factory" });
    const key = "nodekey:" + "b".repeat(64);
    expect(within(dialog).getByLabelText("Device public key")).toHaveValue(key);
    expect(within(dialog).getByLabelText("Device public key")).toHaveAttribute("readonly");
    expect(approve).not.toHaveBeenCalled();
    await user.type(within(dialog).getByLabelText("Label (optional)"), "Demo: Morgan");
    await user.click(within(dialog).getByRole("button", { name: "Approve device" }));
    expect(await screen.findByText(/Demo: Morgan/)).toBeInTheDocument();
    expect(approve).toHaveBeenCalledWith("demo-hosted-factory", 1, "Demo: Morgan");
    expect(screen.queryByText("Incoming join requests")).not.toBeInTheDocument();
  });

  it("dismisses a join request without approving the device", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const dismiss = vi.spyOn(backend, "dismissHostedRequest");
    const approve = vi.spyOn(backend, "approveHostedRequest");
    const { user } = renderApp(<PeopleScreen />, { backend });
    const game = await screen.findByRole("article", { name: "Demo: Factory" });
    await user.click(within(game).getByRole("button", { name: "Dismiss" }));
    expect(await screen.findByText("Join request dismissed.")).toBeInTheDocument();
    expect(dismiss).toHaveBeenCalledWith("demo-hosted-factory", 1);
    expect(approve).not.toHaveBeenCalled();
    expect(within(game).queryByText("Incoming join requests")).not.toBeInTheDocument();
    expect(within(game).getAllByRole("button", { name: "Revoke" })).toHaveLength(1);
  });

  it("copies the selected game's invitation via the native command", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const copy = vi.spyOn(backend, "copyHostedInvitation");
    const { user, container } = renderApp(<PeopleScreen />, { backend });
    await user.click(await screen.findByRole("button", { name: "Copy invitation" }));
    expect(await screen.findByText("Invitation copied. Share it privately.")).toBeInTheDocument();
    expect(copy).toHaveBeenCalledWith("demo-hosted-factory");
    expect(container).not.toHaveTextContent("GameLink demo invitation (example only)");
  });
});

describe.each([
  ["Dashboard", DashboardScreen],
  ["Servers", ServersScreen],
] as const)("%s hosted server cards", (_screen, Screen) => {
  it("copies the selected local and linked games and gives a saved game one card", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const local = await backend.createHostedGame({ name: "Local Bedrock", game: Type.MinecraftBedrock, port: 19132 });
    await backend.configureHost("http://127.0.0.1:8467", "test");
    const snapshot = await backend.refreshHost();
    const linked = snapshot.games[1];
    await backend.importHostGame(linked.id, linked.serverFingerprint ?? "");
    const copyLocal = vi.spyOn(backend, "copyHostedInvitation");
    const copyLinked = vi.spyOn(backend, "copyLinkedInvitation");
    const { user } = renderApp(<><Screen /><Toaster /></>, { backend });

    const linkedCard = await screen.findByRole("article", { name: linked.name });
    await user.click(within(linkedCard).getByRole("button", { name: "Copy invitation" }));
    expect(await screen.findByText(`Invitation copied for ${linked.name}. Share it privately.`)).toBeInTheDocument();
    expect(copyLinked.mock.calls).toEqual([[linked.id, linked.serverFingerprint]]);
    expect(await navigator.clipboard.readText()).toBe(`GameLink demo invitation for ${linked.name} (example only)`);
    expect(within(linkedCard).getByRole("button", { name: "Join" })).toBeInTheDocument();
    expect(copyLocal).not.toHaveBeenCalled();

    const localCard = await screen.findByRole("article", { name: "Local Bedrock" });
    await user.click(within(localCard).getByRole("button", { name: "Copy invitation" }));
    expect(await screen.findByText("Invitation copied for Local Bedrock. Share it privately.")).toBeInTheDocument();
    expect(copyLocal.mock.calls).toEqual([[local.id]]);
    const guestCard = screen.getByRole("article", { name: "Friends Survival" });
    expect(within(guestCard).queryByRole("button", { name: "Copy invitation" })).not.toBeInTheDocument();
  });
});

it("puts hosting controls on the current saved entry when a linked game's tunnel settings change", async () => {
  const backend = createMockBackend({ delayMs: 0, seed: false });
  await backend.configureHost("http://127.0.0.1:8467", "test");
  const snapshot = await backend.refreshHost();
  const game = snapshot.games[0];
  const stale = await backend.importHostGame(game.id, game.serverFingerprint ?? "");
  game.port = 25570;
  vi.spyOn(backend, "hostStatus").mockResolvedValue(snapshot);
  vi.spyOn(backend, "servers").mockResolvedValue([
    { ...stale, name: "Old connection" },
    { ...stale, id: "current", port: game.port, name: "Current connection" },
  ]);
  renderApp(<ServersScreen />, { backend });

  const oldCard = await screen.findByRole("article", { name: "Old connection" });
  const newCard = await screen.findByRole("article", { name: "Current connection" });
  expect(await within(oldCard).findByText(/Saved connection settings are out of date/)).toBeInTheDocument();
  expect(within(oldCard).queryByRole("button", { name: "Copy invitation" })).not.toBeInTheDocument();
  expect(within(newCard).getByRole("button", { name: "Copy invitation" })).toBeInTheDocument();
  expect(screen.queryByRole("article", { name: game.name })).not.toBeInTheDocument();
});

it("retries loading linked games when the host connection becomes available", async () => {
  const backend = createMockBackend({ delayMs: 0, seed: false });
  await backend.configureHost("http://127.0.0.1:8467", "test");
  const snapshot = await backend.refreshHost();
  vi.spyOn(backend, "hostStatus").mockRejectedValueOnce(new Error("HTTP transport request failed"));
  const { user } = renderApp(<ServersScreen />, { backend });

  expect(await screen.findByRole("alert")).toHaveTextContent("Could not load games on your linked node");
  await user.click(screen.getByRole("button", { name: "Retry linked node" }));
  const game = await screen.findByRole("article", { name: snapshot.games[0].name });
  expect(within(game).getByRole("button", { name: "Copy invitation" })).toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

it("shows a LAN world on both screens after sharing and tracks pause, port changes and removal", async () => {
  const backend = createMockBackend({ delayMs: 0 });
  const copy = vi.spyOn(backend, "copyHostedInvitation");
  const scan = vi.spyOn(backend, "scanLanWorlds");
  const user = userEvent.setup();
  render(<App backend={backend} mock />);
  const nav = within(screen.getByRole("navigation", { name: "Main" }));
  const world = "Demo: LAN Cabin";
  const card = () => screen.findByRole("article", { name: world });

  await screen.findByRole("article", { name: "Friends Survival" });
  expect(screen.queryByRole("article", { name: world })).not.toBeInTheDocument();
  await user.click(nav.getByRole("button", { name: "Host" }));
  await user.click(await screen.findByRole("button", { name: "Share world" }));
  await screen.findByText(/Sharing configured for/);

  for (const page of ["Dashboard", "Servers"]) {
    await user.click(nav.getByRole("button", { name: page }));
    const worldCard = await card();
    expect(worldCard).toHaveTextContent("Hosted on this PC · Port 51734");
    expect(within(worldCard).getByRole("button", { name: "Copy invitation" })).toBeInTheDocument();
  }
  await user.click(within(await card()).getByRole("button", { name: "Copy invitation" }));
  expect(await screen.findByText(`Invitation copied for ${world}. Share it privately.`)).toBeInTheDocument();
  expect(copy).toHaveBeenCalledWith("demo-hosted-1");

  await user.click(nav.getByRole("button", { name: "Host" }));
  const hostedWorld = async () => within((await screen.findByRole("heading", { name: world, level: 3 })).closest("li")!);
  await user.click((await hostedWorld()).getByRole("button", { name: "Stop sharing" }));
  await (await hostedWorld()).findByText(/Sharing paused/);
  await user.click(nav.getByRole("button", { name: "Dashboard" }));
  const paused = await card();
  expect(paused).toHaveTextContent("Sharing paused");
  expect(within(paused).queryByRole("button", { name: "Copy invitation" })).not.toBeInTheDocument();

  scan.mockResolvedValue([{ name: world, port: 51800 }]);
  await user.click(nav.getByRole("button", { name: "Host" }));
  await user.click(await screen.findByRole("button", { name: "Update LAN port" }));
  await screen.findByText(/Sharing configured for/);
  await user.click(nav.getByRole("button", { name: "Servers" }));
  const updated = await card();
  expect(updated).toHaveTextContent("Port 51800");
  await user.click(within(updated).getByRole("button", { name: "Copy invitation" }));
  expect(copy.mock.calls).toEqual([["demo-hosted-1"], ["demo-hosted-1"]]);

  await user.click(nav.getByRole("button", { name: "Host" }));
  await user.click((await hostedWorld()).getByRole("button", { name: "Remove" }));
  await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove game" }));
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  for (const page of ["Dashboard", "Servers"]) {
    await user.click(nav.getByRole("button", { name: page }));
    await screen.findByRole("article", { name: "Demo: Factory" });
    expect(screen.queryByRole("article", { name: world })).not.toBeInTheDocument();
  }
});

it("shows a linked node with no cached games as refreshing in People", async () => {
  const backend = createMockBackend({ delayMs: 0 });
  const refreshing = { games: [], refreshedAt: "", refreshing: true };
  vi.spyOn(backend, "hostStatus").mockResolvedValue(refreshing);
  renderApp(<PeopleScreen />, { backend });

  const linked = await screen.findByRole("region", { name: "Access to games on your linked node" });
  expect(within(linked).getByRole("status")).toHaveTextContent("Refreshing…");
  expect(within(linked).queryByRole("alert")).not.toBeInTheDocument();
});

it("reviews and explicitly approves the selected device for a linked game in People", async () => {
  const backend = createMockBackend({ delayMs: 0 });
  await backend.configureHost("http://127.0.0.1:8467", "test");
  const snapshot = await backend.refreshHost();
  const game = snapshot.games[1];
  const request = { id: 21, publicKey: "nodekey:" + "e".repeat(64), lastSeen: new Date().toISOString() };
  game.pending = [request];
  vi.spyOn(backend, "hostStatus").mockResolvedValue(snapshot);
  const approve = vi.spyOn(backend, "approveLinkedRequest").mockResolvedValue();
  const approveLocal = vi.spyOn(backend, "approveHostedRequest");
  const { user } = renderApp(<PeopleScreen />, { backend });

  const card = await screen.findByRole("article", { name: game.name });
  await user.click(within(card).getByRole("button", { name: "Review request" }));
  const dialog = screen.getByRole("dialog", { name: `Approve a device for ${game.name}` });
  expect(within(dialog).getByLabelText("Device public key")).toHaveValue(request.publicKey);
  expect(approve).not.toHaveBeenCalled();
  await user.click(within(dialog).getByRole("button", { name: "Approve device" }));
  expect(await screen.findByText(`Device approved for ${game.name}. Ask your friend to try Join again.`)).toBeInTheDocument();
  expect(approve.mock.calls).toEqual([[game.id, game.serverFingerprint, request, ""]]);
  expect(approveLocal).not.toHaveBeenCalled();
});

it("dismisses a linked game's join request on the node without approving it", async () => {
  const backend = createMockBackend({ delayMs: 0 });
  await backend.configureHost("http://127.0.0.1:8467", "test");
  const snapshot = await backend.refreshHost();
  const game = snapshot.games[1];
  const request = { id: 22, publicKey: "nodekey:" + "a".repeat(64), lastSeen: new Date().toISOString() };
  game.pending = [request];
  const status = vi.spyOn(backend, "hostStatus").mockResolvedValue(snapshot);
  const dismiss = vi.spyOn(backend, "dismissLinkedRequest").mockImplementation(async () => {
    status.mockResolvedValue({ ...snapshot, games: snapshot.games.map((g) => (g.id === game.id ? { ...g, pending: [] } : g)) });
  });
  const approve = vi.spyOn(backend, "approveLinkedRequest");
  const { user } = renderApp(<PeopleScreen />, { backend });

  const card = await screen.findByRole("article", { name: game.name });
  await user.click(within(card).getByRole("button", { name: "Dismiss" }));
  expect(await screen.findByText("Join request dismissed.")).toBeInTheDocument();
  expect(dismiss.mock.calls).toEqual([[game.id, game.serverFingerprint, request]]);
  expect(approve).not.toHaveBeenCalled();
  expect(within(card).queryByText("Incoming join requests")).not.toBeInTheDocument();
});
