import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { HostScreen } from "./HostScreen";
import { createMockBackend } from "../lib/mockBackend";
import { renderApp } from "../test/utils";
import type { HostSnapshot } from "../lib/types";

describe("HostScreen", () => {
  it("saves the token, refreshes only on demand, and imports a selected game", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const initialRefresh = vi.spyOn(backend, "refreshHost");
    const cachedStatus = vi.spyOn(backend, "hostStatus");
    renderApp(<HostScreen />, { backend });
    await screen.findByText("Save a host connection to discover its games.");
    expect(initialRefresh).not.toHaveBeenCalled();
    expect(cachedStatus).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText("Host endpoint"), { target: { value: "https://host.example" } });
    fireEvent.change(screen.getByLabelText("Administrator token"), { target: { value: "test-token" } });
    fireEvent.click(screen.getByRole("button", { name: "Save connection" }));
    await screen.findByText("Host connection saved on this device.");
    expect(screen.getByLabelText("Administrator token")).toHaveValue("");
    expect(initialRefresh).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Refresh games" }));
    expect(await screen.findByText("Friends Minecraft")).toBeInTheDocument();
    expect(screen.getByText("Unknown")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Add to GameLink" })[0]);
    expect(await screen.findByText("Friends Minecraft was added to your servers.")).toBeInTheDocument();
  });

  it("recovers saved Panel inventory on demand and switches to another saved host without a token", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const first = "https://one.example";
    const second = "https://two.example";
    backend.hostConnection = vi.fn().mockResolvedValue({ endpoint: first, configured: true, savedHosts: [{ endpoint: first, name: "" }, { endpoint: second, name: "Wings 4" }] });
    backend.configureHost = vi.fn().mockResolvedValue({ endpoint: second, configured: true, savedHosts: [{ endpoint: second, name: "Wings 4" }, { endpoint: first, name: "" }] });
    const panel = { endpoint: "https://panel.example", nodeIds: [2, 4] };
    backend.panelConnection = vi.fn().mockResolvedValue(panel);
    backend.refreshPanelInventory = vi.fn()
      .mockResolvedValueOnce({
        connection: panel,
        servers: [{ id: 6, name: "Factorio test", nodeId: 2, suspended: false }],
      })
      .mockResolvedValueOnce({
        connection: { endpoint: "https://restored.example", nodeIds: [7] },
        servers: [{ id: 8, name: "Restored game", nodeId: 7, suspended: false }],
      });
    renderApp(<HostScreen />, { backend });

    expect(await screen.findByText("Saved nodes 2, 4")).toBeInTheDocument();
    expect(screen.getByText("https://panel.example")).toBeInTheDocument();
    expect(screen.getByText(/enter the GameLink agent endpoint and administrator token/)).toBeInTheDocument();
    expect(backend.refreshPanelInventory).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Refresh inventory" }));
    expect(await screen.findByText("Factorio test")).toBeInTheDocument();
    expect(screen.getByText("Node 4")).toBeInTheDocument();
    expect(screen.getByText("No servers on this node.")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Active saved host"), { target: { value: second } });
    await waitFor(() => expect(screen.getByLabelText("Host endpoint")).toHaveValue(second));
    expect(backend.configureHost).toHaveBeenCalledWith(second, "");
    expect(screen.getByText("Factorio test")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Refresh inventory" }));
    expect(await screen.findByText("Restored game")).toBeInTheDocument();
    expect(screen.getByText("https://restored.example")).toBeInTheDocument();
    expect(screen.getByText("Node 7")).toBeInTheDocument();
    expect(screen.queryByText("Node 2")).not.toBeInTheDocument();
  });

  it("shows read errors and prevents overwriting a malformed host profile", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    backend.hostConnection = vi.fn().mockRejectedValue("Restore host-connection.json from a backup, then reload.");
    backend.panelConnection = vi.fn().mockRejectedValue("Restore panel-connection.json from a backup, then reload.");
    const save = vi.spyOn(backend, "configureHost");
    renderApp(<HostScreen />, { backend });

    expect(await screen.findByText(/Restore host-connection.json/)).toHaveAttribute("role", "alert");
    expect(await screen.findByText(/Restore panel-connection.json/)).toHaveAttribute("role", "alert");
    fireEvent.change(screen.getByLabelText("Host endpoint"), { target: { value: "https://new.example" } });
    expect(screen.getByRole("button", { name: "Save connection" })).toBeDisabled();
    expect(save).not.toHaveBeenCalled();
  });

  it("shows refresh errors and keeps the refresh action busy until the request settles", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    await backend.configureHost("https://host.example", "token");
    let finish!: (value: never) => void;
    backend.refreshHost = vi.fn(() => new Promise<HostSnapshot>((_, reject) => {
      finish = () => reject(new Error("host request failed (HTTP 503)"));
    }));
    renderApp(<HostScreen />, { backend });
    const refresh = await screen.findByRole("button", { name: "Refresh games" });
    fireEvent.click(refresh);
    await waitFor(() => expect(refresh).toBeDisabled());
    finish(undefined as never);
    expect(await screen.findByRole("alert")).toHaveTextContent("host request failed (HTTP 503)");
    expect(refresh).toBeEnabled();
  });
});
