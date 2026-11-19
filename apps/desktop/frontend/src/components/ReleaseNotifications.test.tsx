import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { createMockBackend } from "../lib/mockBackend";
import { GameLinkProvider, useStore } from "../lib/store";
import { ReleaseNotifications } from "./ReleaseNotifications";
import type { AgentUpdateProgress, ReleaseNotice } from "../lib/types";

const update: ReleaseNotice = {
  version: "0.3.0",
  severity: "normal",
  releaseNotesUrl: "https://example.org/releases/0.3.0",
  summary: "Improves connection recovery.",
  canInstall: true,
  installerError: null,
  linkedAgent: null,
};

const linkedUpdate: ReleaseNotice = {
  ...update,
  linkedAgent: { kind: "available", nodeName: "games.example.org", offerId: "old-offer" },
};
const manualAgentUpdate = "Update the GameLink agent on your server with the setup guide or your package manager.";

function setupLinkedUpdate(notice = linkedUpdate) {
  const backend = createMockBackend({ delayMs: 0 });
  vi.spyOn(backend, "checkRelease").mockResolvedValue(notice);
  const server = vi.spyOn(backend, "updateLinkedAgent");
  const app = vi.spyOn(backend, "installUpdate").mockResolvedValue(undefined);
  const relaunch = vi.spyOn(backend, "relaunch");
  return { backend, server, app, relaunch, user: userEvent.setup() };
}

describe("Release notifications", () => {
  it("shows plain Update when no node is linked, tracks installation, and relaunches", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    vi.spyOn(backend, "checkRelease").mockResolvedValue(update);
    const server = vi.spyOn(backend, "updateLinkedAgent");
    let finish!: () => void;
    const install = vi.spyOn(backend, "installUpdate").mockImplementation((_version, onProgress) => {
      onProgress({ stage: "downloading", downloaded: 25, total: 100 });
      return new Promise<void>((resolve) => {
        finish = () => {
          onProgress({ stage: "installing" });
          resolve();
        };
      });
    });
    const relaunch = vi.spyOn(backend, "relaunch");
    const user = userEvent.setup();

    render(<App backend={backend} />);
    const button = await screen.findByRole("button", { name: "Update" });
    expect(button.closest("main")).toBeNull();
    expect(screen.queryByRole("button", { name: "Update all" })).not.toBeInTheDocument();
    expect(screen.getByText("Update available · GameLink 0.3.0")).toBeInTheDocument();
    expect(screen.getByText(update.summary!)).toBeInTheDocument();
    await user.click(button);
    expect(install).toHaveBeenCalledWith(update.version, expect.any(Function), undefined);
    expect(server).not.toHaveBeenCalled();
    expect(screen.getByRole("progressbar", { name: "Downloading update" })).toHaveAttribute("value", "25");
    expect(screen.getByRole("button", { name: "Updating…" })).toBeDisabled();
    expect(relaunch).not.toHaveBeenCalled();
    await act(async () => finish());
    expect(relaunch).toHaveBeenCalledOnce();
    expect(screen.getByText("Restarting GameLink…")).toBeInTheDocument();
  });

  it("updates the linked server first and installs the app only after server success", async () => {
    const { backend, server, app, relaunch, user } = setupLinkedUpdate();
    let progress!: (phase: AgentUpdateProgress) => void;
    let succeed!: () => void;
    server.mockImplementation((_version, _offerId, onProgress) => {
      progress = onProgress;
      return new Promise<void>((resolve) => { succeed = resolve; });
    });

    render(<App backend={backend} />);
    const button = await screen.findByRole("button", { name: "Update all" });
    expect(screen.getByText("Update this PC and your server games.example.org.")).toBeInTheDocument();
    expect(server).not.toHaveBeenCalled();
    expect(app).not.toHaveBeenCalled();

    await user.click(button);
    expect(server).toHaveBeenCalledWith(update.version, "old-offer", expect.any(Function));
    expect(screen.getByText("Preparing the server update…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update this app only" })).toBeDisabled();
    for (const [phase, text] of [
      ["installing", "Installing the server update…"],
      ["restarting", "Waiting for your server agent to restart…"],
      ["checkingHealth", "Checking server agent health…"],
    ] as const) {
      act(() => progress(phase));
      expect(screen.getByText(text)).toBeInTheDocument();
      expect(app).not.toHaveBeenCalled();
      expect(relaunch).not.toHaveBeenCalled();
    }

    await act(async () => succeed());
    expect(app).toHaveBeenCalledExactlyOnceWith(update.version, expect.any(Function), "old-offer");
    expect(relaunch).toHaveBeenCalledOnce();
    act(() => progress("checkingHealth"));
    expect(screen.getByText("Restarting GameLink…")).toBeInTheDocument();
  });

  it("stops on server rollback, shows its reason, and lets the user retry", async () => {
    const { backend, server, app, relaunch, user } = setupLinkedUpdate();
    const rollback = "The server rolled back the agent update. The service failed its health check.";
    server.mockRejectedValueOnce({ kind: "failed", message: rollback }).mockResolvedValueOnce(undefined);

    render(<App backend={backend} />);
    await user.click(await screen.findByRole("button", { name: "Update all" }));
    expect(await screen.findByText(rollback)).toHaveAttribute("role", "alert");
    expect(app).not.toHaveBeenCalled();
    expect(relaunch).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Update this app only" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(server).toHaveBeenCalledTimes(2);
    expect(app).toHaveBeenCalledOnce();
  });

  it.each(["succeeds", "fails"])("refreshes a relinked server and waits for a fresh click when the old update %s", async (outcome) => {
    const { backend, server, app, relaunch, user } = setupLinkedUpdate();
    const check = vi.mocked(backend.checkRelease).mockResolvedValueOnce(linkedUpdate);
    let finish!: () => void;
    server.mockResolvedValue(undefined).mockImplementationOnce(() => new Promise<void>((resolve, reject) => {
      finish = () => outcome === "succeeds" ? resolve() : reject({ kind: "failed", message: "Old server failed." });
    }));
    function Relink() {
      const store = useStore();
      return <button onClick={() => void store.configureHost("https://new.example.org", "test-token")}>
        Relink
      </button>;
    }
    render(<GameLinkProvider backend={backend}><ReleaseNotifications /><Relink /></GameLinkProvider>);
    await user.click(await screen.findByRole("button", { name: "Update all" }));
    check.mockResolvedValue({
      ...linkedUpdate,
      linkedAgent: { kind: "available", nodeName: "new.example.org", offerId: "new-offer" },
    });

    await user.click(screen.getByRole("button", { name: "Relink" }));
    expect(check).toHaveBeenCalledOnce();
    await act(async () => finish());

    expect(app).not.toHaveBeenCalled();
    expect(relaunch).not.toHaveBeenCalled();
    expect(await screen.findByText("Update this PC and your server new.example.org.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
    expect(server).toHaveBeenCalledOnce();
    await user.click(screen.getByRole("button", { name: "Update all" }));
    expect(server).toHaveBeenCalledTimes(2);
    expect(server).toHaveBeenLastCalledWith(update.version, "new-offer", expect.any(Function));
  });

  it("updates only the app when selected", async () => {
    const { backend, server, app, relaunch, user } = setupLinkedUpdate();
    render(<App backend={backend} />);
    await user.click(await screen.findByRole("button", { name: "Update this app only" }));
    expect(server).not.toHaveBeenCalled();
    expect(app).toHaveBeenCalledExactlyOnceWith(update.version, expect.any(Function), undefined);
    expect(relaunch).toHaveBeenCalledOnce();
  });

  it("retries the desktop installer without requesting a second server update", async () => {
    const { backend, server, app, user } = setupLinkedUpdate();
    server.mockResolvedValueOnce(undefined);
    app.mockRejectedValueOnce("Could not download the installer.");
    render(<App backend={backend} />);
    await user.click(await screen.findByRole("button", { name: "Update all" }));
    expect(await screen.findByText("Could not download the installer.")).toHaveAttribute("role", "alert");
    await user.click(screen.getByRole("button", { name: "Retry update" }));
    expect(server).toHaveBeenCalledOnce();
    expect(app).toHaveBeenCalledTimes(2);
    expect(app).toHaveBeenNthCalledWith(2, update.version, expect.any(Function), "old-offer");
  });

  it.each([
    "Try again in a few minutes.",
    "Timed out waiting for your server to finish updating. This app has not been updated.",
  ])("leaves the app untouched when the server reports %s", async (message) => {
    const { backend, server, app, relaunch, user } = setupLinkedUpdate();
    server.mockRejectedValue({ kind: "failed", message });
    render(<App backend={backend} />);
    await user.click(await screen.findByRole("button", { name: "Update all" }));
    expect(await screen.findByText(message)).toHaveAttribute("role", "alert");
    expect(screen.queryByText("Preparing the server update…")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry" })).toBeEnabled();
    expect(server).toHaveBeenCalledOnce();
    expect(app).not.toHaveBeenCalled();
    expect(relaunch).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Update this app only" }));
    expect(server).toHaveBeenCalledOnce();
    expect(app).toHaveBeenCalledOnce();
  });

  it("shows manual guidance when the linked agent lacks self-update support", async () => {
    const { backend, server } = setupLinkedUpdate({ ...update, linkedAgent: { kind: "manual" } });
    render(<App backend={backend} />);
    expect(await screen.findByText(manualAgentUpdate)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update this app only" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Update all" })).not.toBeInTheDocument();
    expect(server).not.toHaveBeenCalled();
  });

  it("falls back to the manual guidance when the update route becomes unavailable", async () => {
    const { backend, server, app, user } = setupLinkedUpdate();
    server.mockRejectedValue({ kind: "manual", message: manualAgentUpdate });
    render(<App backend={backend} />);
    await user.click(await screen.findByRole("button", { name: "Update all" }));
    expect(await screen.findByText(manualAgentUpdate)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update this app only" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Retry" })).not.toBeInTheDocument();
    expect(app).not.toHaveBeenCalled();
  });

  it("stays quiet when no verified release is returned", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const check = vi.spyOn(backend, "checkRelease");
    render(<App backend={backend} />);
    await waitFor(() => expect(check).toHaveBeenCalledOnce());
    expect(screen.queryByRole("button", { name: "Update" })).not.toBeInTheDocument();
    expect(screen.queryByRole("complementary", { name: "GameLink updates" })).not.toBeInTheDocument();
  });

  it("checks again on the four-hour timer", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const check = vi.spyOn(backend, "checkRelease").mockResolvedValueOnce(null).mockResolvedValueOnce(update);
    const interval = vi.spyOn(globalThis, "setInterval");
    try {
      render(<App backend={backend} />);
      await waitFor(() => expect(check).toHaveBeenCalledOnce());
      const tick = interval.mock.calls.find(([, delay]) => delay === 4 * 60 * 60 * 1000)?.[0];
      if (typeof tick !== "function") throw new Error("No periodic update check was scheduled");
      await act(async () => tick());
      expect(await screen.findByRole("button", { name: "Update" })).toBeInTheDocument();
      expect(check).toHaveBeenCalledTimes(2);
    } finally {
      interval.mockRestore();
    }
  });

  it("shows an install error and can retry", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    vi.spyOn(backend, "checkRelease").mockResolvedValue(update);
    const install = vi.spyOn(backend, "installUpdate")
      .mockRejectedValueOnce("Could not download the installer. Check your connection and try again.")
      .mockResolvedValueOnce(undefined);
    const relaunch = vi.spyOn(backend, "relaunch");
    const user = userEvent.setup();

    render(<App backend={backend} />);
    await user.click(await screen.findByRole("button", { name: "Update" }));
    expect(await screen.findByText("Could not download the installer. Check your connection and try again."))
      .toHaveAttribute("role", "alert");
    expect(relaunch).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Retry update" }));
    expect(install).toHaveBeenCalledTimes(2);
    await waitFor(() => expect(relaunch).toHaveBeenCalledOnce());
  });

  it("retries only the restart when launching the installed update fails", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    vi.spyOn(backend, "checkRelease").mockResolvedValue(update);
    const install = vi.spyOn(backend, "installUpdate").mockResolvedValue(undefined);
    const relaunch = vi.spyOn(backend, "relaunch")
      .mockRejectedValueOnce("Could not restart GameLink. Try restarting again.")
      .mockResolvedValueOnce(undefined);
    const user = userEvent.setup();

    render(<App backend={backend} />);
    await user.click(await screen.findByRole("button", { name: "Update" }));
    expect(await screen.findByText("Could not restart GameLink. Try restarting again."))
      .toHaveAttribute("role", "alert");
    await user.click(screen.getByRole("button", { name: "Restart GameLink" }));
    expect(relaunch).toHaveBeenCalledTimes(2);
    expect(install).toHaveBeenCalledOnce();
  });

  it("keeps the notes link and retries when the installer feed is unavailable", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    vi.spyOn(backend, "checkRelease")
      .mockResolvedValueOnce({ ...update, canInstall: false, installerError: "The installer is unavailable." })
      .mockResolvedValueOnce(update);
    const user = userEvent.setup();

    render(<App backend={backend} />);
    expect(await screen.findByText("The installer is unavailable.")).toHaveAttribute("role", "alert");
    expect(screen.getByRole("link", { name: "Release notes" })).toHaveAttribute("href", update.releaseNotesUrl);
    await user.click(screen.getByRole("button", { name: "Retry check" }));
    expect(await screen.findByRole("button", { name: "Update" })).toBeInTheDocument();
  });

  it("emphasizes a verified security release", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    vi.spyOn(backend, "checkRelease").mockResolvedValue({ ...update, severity: "security" });
    render(<App backend={backend} />);
    const notice = await screen.findByText("Security update available · GameLink 0.3.0");
    expect(notice.closest("aside")).toHaveClass("border-warn/60");
    expect(screen.getByRole("button", { name: "Update" })).toBeInTheDocument();
  });

  it("honors an existing opt-out and offers manual notes on builds without install support", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    await backend.updateSettings({ ...(await backend.settings()), checkForUpdates: false });
    const check = vi.spyOn(backend, "checkRelease").mockResolvedValue({ ...update, canInstall: false, summary: null });
    const open = vi.spyOn(backend, "openReleaseNotes");
    const user = userEvent.setup();

    render(<App backend={backend} />);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const toggle = await screen.findByRole("switch", { name: "Check for updates" });
    expect(toggle).not.toBeChecked();
    expect(check).not.toHaveBeenCalled();

    await user.click(toggle);
    const link = await screen.findByRole("link", { name: "Release notes" });
    expect(link).toHaveAttribute("href", update.releaseNotesUrl);
    expect(screen.queryByRole("button", { name: "Update" })).not.toBeInTheDocument();
    await user.click(link);
    expect(open).toHaveBeenCalledOnce();

    await user.click(toggle);
    await waitFor(() => expect(link).not.toBeInTheDocument());
  });

  it("does not check for updates without a release verification pin", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const settings = await backend.settings();
    vi.spyOn(backend, "settings").mockResolvedValue({ ...settings, canCheckForUpdates: false });
    const check = vi.spyOn(backend, "checkRelease");
    const user = userEvent.setup();

    render(<App backend={backend} />);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByRole("switch", { name: "Check for updates" })).toBeDisabled();
    expect(check).not.toHaveBeenCalled();
    expect(screen.queryByRole("complementary", { name: "GameLink updates" })).not.toBeInTheDocument();
  });
});
