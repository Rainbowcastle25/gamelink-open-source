import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { createMockBackend } from "../lib/mockBackend";
import { renderApp } from "../test/utils";
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
