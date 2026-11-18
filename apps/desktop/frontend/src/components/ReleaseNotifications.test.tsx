import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import App from "../App";
import { createMockBackend } from "../lib/mockBackend";
import type { ReleaseNotice } from "../lib/types";

const update: ReleaseNotice = {
  version: "0.3.0",
  severity: "normal",
  releaseNotesUrl: "https://example.org/releases/0.3.0",
};

describe("Release notifications", () => {
  it("checks only after opt-in and hides the banner when switched off", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    const check = vi.spyOn(backend, "checkRelease").mockResolvedValue(update);
    const open = vi.spyOn(backend, "openReleaseNotes");
    const user = userEvent.setup();

    render(<App backend={backend} />);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    const toggle = await screen.findByRole("switch", { name: "Check for updates" });
    expect(toggle).not.toBeChecked();
    expect(check).not.toHaveBeenCalled();
    expect(screen.queryByRole("link", { name: "Release notes" })).not.toBeInTheDocument();

    await user.click(toggle);
    expect(await screen.findByText("Update available · GameLink 0.3.0")).toBeInTheDocument();
    const link = screen.getByRole("link", { name: "Release notes" });
    expect(link).toHaveAttribute("href", update.releaseNotesUrl);
    await user.click(link);
    expect(open).toHaveBeenCalledOnce();

    await user.click(toggle);
    await waitFor(() => expect(link).not.toBeInTheDocument());
  });

  it("emphasizes a verified security release", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    await backend.updateSettings({ ...(await backend.settings()), checkForUpdates: true });
    vi.spyOn(backend, "checkRelease").mockResolvedValue({ ...update, severity: "security" });
    render(<App backend={backend} />);
    const notice = await screen.findByText("Security update available · GameLink 0.3.0");
    expect(notice.closest("aside")).toHaveClass("border-warn/60");
  });

  it("stays quiet when no verified release is returned", async () => {
    const backend = createMockBackend({ delayMs: 0 });
    await backend.updateSettings({ ...(await backend.settings()), checkForUpdates: true });
    const check = vi.spyOn(backend, "checkRelease");
    render(<App backend={backend} />);
    await waitFor(() => expect(check).toHaveBeenCalled());
    expect(screen.queryByRole("link", { name: "Release notes" })).not.toBeInTheDocument();
  });
});
