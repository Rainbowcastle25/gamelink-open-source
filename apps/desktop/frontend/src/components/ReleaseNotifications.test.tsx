import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { createMockBackend } from "../lib/mockBackend";
import type { ReleaseNotice } from "../lib/types";

const update: ReleaseNotice = {
  version: "0.3.0",
  severity: "normal",
  releaseNotesUrl: "https://example.org/releases/0.3.0",
  summary: "Improves connection recovery.",
  canInstall: true,
  installerError: null,
};

describe("Release notifications", () => {
  it("shows the update from startup, tracks installation, and relaunches", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    vi.spyOn(backend, "checkRelease").mockResolvedValue(update);
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
    expect(screen.getByText("Update available · GameLink 0.3.0")).toBeInTheDocument();
    expect(screen.getByText(update.summary!)).toBeInTheDocument();
    await user.click(button);
    expect(install).toHaveBeenCalledWith(update.version, expect.any(Function));
    expect(screen.getByRole("progressbar", { name: "Downloading update" })).toHaveAttribute("value", "25");
    expect(screen.getByRole("button", { name: "Updating…" })).toBeDisabled();
    expect(relaunch).not.toHaveBeenCalled();
    await act(async () => finish());
    expect(relaunch).toHaveBeenCalledOnce();
    expect(screen.getByText("Restarting GameLink…")).toBeInTheDocument();
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
