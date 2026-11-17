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
