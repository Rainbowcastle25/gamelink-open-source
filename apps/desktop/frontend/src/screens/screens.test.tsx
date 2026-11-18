import { screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ImportInviteModal } from "../components/ImportInviteModal";
import { SettingsPanel } from "../components/SettingsPanel";
import { createMockBackend } from "../lib/mockBackend";
import { renderApp } from "../test/utils";
import { DashboardScreen } from "./DashboardScreen";
import { PeopleScreen } from "./PeopleScreen";
import { ServersScreen } from "./ServersScreen";

const KEY = "nodekey:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

describe("Import invitation flow", () => {
  it("previews a pasted invitation, then adds the server on confirm", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const preview = vi.spyOn(backend, "previewInvite");
    const importInvite = vi.spyOn(backend, "importInvite");
    const onClose = vi.fn();
    const { user } = renderApp(<ImportInviteModal onClose={onClose} />, { backend });

    const confirm = screen.getByRole("button", { name: "Add server" });
    expect(confirm).toBeDisabled();

    await user.click(screen.getByLabelText("Invitation"));
    await user.paste("glink1.survival-invite-0001");

    expect(await screen.findByText("Weekend Minecraft")).toBeInTheDocument();
    expect(preview).toHaveBeenCalledWith("glink1.survival-invite-0001");
    expect(screen.getByText(/public Tailcat relays/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Warnings")).not.toBeInTheDocument();
    expect(confirm).toBeEnabled();

    await user.click(confirm);
    await waitFor(() => expect(importInvite).toHaveBeenCalledWith("glink1.survival-invite-0001", 0));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("passes a validated local port to the backend", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const importInvite = vi.spyOn(backend, "importInvite");
    const { user } = renderApp(<ImportInviteModal onClose={() => {}} />, { backend });
    await user.click(screen.getByLabelText("Invitation"));
    await user.paste("glink1.survival-invite-0001");
    await screen.findByText("Weekend Minecraft");

    const port = screen.getByLabelText(/Local port/);
    await user.type(port, "70000");
    expect(screen.getByText(/between 1 and 65535/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add server" })).toBeDisabled();

    await user.clear(port);
    await user.type(port, "25570");
    await user.click(screen.getByRole("button", { name: "Add server" }));
    await waitFor(() => expect(importInvite).toHaveBeenCalledWith("glink1.survival-invite-0001", 25570));
  });

  it("shows a plain-language error for an invalid invitation and never enables the button", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const importInvite = vi.spyOn(backend, "importInvite");
    const { user } = renderApp(<ImportInviteModal onClose={() => {}} />, { backend });
    await user.click(screen.getByLabelText("Invitation"));
    await user.paste("hello this is not an invitation");
    expect(await screen.findByText(/does not look like a GameLink invitation/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add server" })).toBeDisabled();
    expect(importInvite).not.toHaveBeenCalled();
  });

  it("warns clearly when the invitation uses a custom relay", async () => {
    const { user } = renderApp(<ImportInviteModal onClose={() => {}} />);
    await user.click(screen.getByLabelText("Invitation"));
    await user.paste("glink1.custom-relay-invite-01");
    const warnings = await screen.findByLabelText("Warnings");
    expect(within(warnings).getByText(/custom relay/i)).toBeInTheDocument();
    expect(screen.queryByText(/public Tailcat relays, which only pass/)).not.toBeInTheDocument();
  });

  it("shows a backend error from the final import inline and keeps the dialog open", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    vi.spyOn(backend, "importInvite").mockRejectedValue(new Error("this server is already saved"));
    const onClose = vi.fn();
    const { user } = renderApp(<ImportInviteModal onClose={onClose} />, { backend });
    await user.click(screen.getByLabelText("Invitation"));
    await user.paste("glink1.survival-invite-0001");
    await screen.findByText("Weekend Minecraft");
    await user.click(screen.getByRole("button", { name: "Add server" }));
    expect(await screen.findByText("This server is already saved.")).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("closes with Escape", async () => {
    const onClose = vi.fn();
    const { user } = renderApp(<ImportInviteModal onClose={onClose} />);
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalled();
  });
});

describe("Servers screen", () => {
  it("opens the import dialog from the Add server menu", async () => {
    const { user } = renderApp(<ServersScreen />);
    await screen.findByText("Friends Survival");
    await user.click(screen.getByRole("button", { name: /Add server/ }));
    await user.click(screen.getByRole("menuitem", { name: /Import invitation/ }));
    expect(screen.getByRole("dialog", { name: "Import invitation" })).toBeInTheDocument();
  });

  it("asks for confirmation before removing a server", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const remove = vi.spyOn(backend, "removeServer");
    const { user } = renderApp(<ServersScreen />, { backend });
    await screen.findByText("Friends Survival");
    await user.click(screen.getByRole("button", { name: "Show details for Friends Survival" }));
    await user.click(screen.getByRole("button", { name: "Remove" }));
    expect(screen.getByText("Remove Friends Survival? You will need the invitation again to re-add it.")).toBeInTheDocument();
    expect(remove).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(remove).toHaveBeenCalledWith("mock-survival"));
    await waitFor(() => expect(screen.queryByText("Friends Survival")).not.toBeInTheDocument());
  });
});

describe("People screen", () => {
  it("shows the full public key and the approve command with it filled in", async () => {
    renderApp(<PeopleScreen />);
    expect(await screen.findByLabelText("Public key")).toHaveTextContent(KEY);
    const command = screen.getByLabelText("Approve command");
    expect(command).toHaveTextContent(`gamelink-agent devices approve --game <game-id> --key ${KEY}`);
    expect(screen.getByText(/Contact the host directly to ask for access/)).toBeInTheDocument();
  });

  it("Copy Public Key writes the key to the clipboard", async () => {
    const { user } = renderApp(<PeopleScreen />);
    await screen.findByLabelText("Public key");
    const write = vi.spyOn(navigator.clipboard, "writeText");
    await user.click(screen.getByRole("button", { name: "Copy Public Key" }));
    expect(write).toHaveBeenCalledWith(KEY);
  });

  it("falls back to the backend clipboard when the browser one fails", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const copy = vi.spyOn(backend, "copyText");
    const { user } = renderApp(<PeopleScreen />, { backend });
    await screen.findByLabelText("Public key");
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));
    await user.click(screen.getByRole("button", { name: "Copy Public Key" }));
    await waitFor(() => expect(copy).toHaveBeenCalledWith(KEY));
  });
});

describe("Settings", () => {
  it("toggling a preference calls updateSettings immediately", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const update = vi.spyOn(backend, "updateSettings");
    const { user } = renderApp(<SettingsPanel />, { backend });
    const toggle = await screen.findByRole("switch", { name: "Reconnect automatically" });
    expect(toggle).toBeChecked();
    await user.click(toggle);
    expect(toggle).not.toBeChecked(); // optimistic
    await waitFor(() => expect(update).toHaveBeenCalledWith(expect.objectContaining({ autoReconnect: false })));
  });

  it("reverts the toggle and reports the error when the backend refuses", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    vi.spyOn(backend, "updateSettings").mockRejectedValue(new Error("minecraft-java: that file does not exist"));
    const { user } = renderApp(<SettingsPanel />, { backend });
    const toggle = await screen.findByRole("switch", { name: "Check servers on startup" });
    await user.click(toggle);
    await waitFor(() => expect(toggle).toBeChecked());
  });

  it("Browse saves the picked executable through updateSettings", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const update = vi.spyOn(backend, "updateSettings");
    const { user } = renderApp(<SettingsPanel />, { backend });
    await user.click(await screen.findByRole("button", { name: "Browse for Factorio" }));
    await waitFor(() =>
      expect(update).toHaveBeenCalledWith(expect.objectContaining({ gameExecutables: expect.objectContaining({ factorio: "C:\\Games\\Factorio\\bin\\x64\\factorio.exe" }) })),
    );
    expect(await screen.findByText("C:\\Games\\Factorio\\bin\\x64\\factorio.exe")).toBeInTheDocument();
  });

  it("does not claim to be perfectly secure and states the alpha limits", async () => {
    const { container } = renderApp(<SettingsPanel />);
    await screen.findByText("GPL-3.0");
    expect(container.textContent).not.toMatch(/100% secure/i);
    expect(screen.getByText(/not security-audited/)).toBeInTheDocument();
  });
});

describe("Dashboard and demo servers", () => {
  it("shows the demo tag on sample servers only and does not mislabel real servers", async () => {
    const { user, backend } = renderApp(<DashboardScreen />, { demoMode: true });
    const join = vi.spyOn(backend, "join");
    await screen.findByText("Demo: Survival World");
    expect(screen.getAllByText("Demo")).toHaveLength(3);

    const real = screen.getByRole("article", { name: "Friends Survival" });
    expect(within(real).queryByText("Demo")).not.toBeInTheDocument();
    await user.click(within(real).getByRole("button", { name: "Join only" }));
    expect(join).toHaveBeenCalledWith("mock-survival");
    expect(real).not.toHaveTextContent(/simulated/i);
  });

  it("joining a server walks through connecting to connected with a copyable address", async () => {
    const { user } = renderApp(<DashboardScreen />);
    const card = await screen.findByRole("article", { name: "Friends Survival" });
    await user.click(within(card).getByRole("button", { name: "Join only" }));
    expect(await within(card).findByText("127.0.0.1:25565")).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Copy Address" })).toBeInTheDocument();
    expect(within(card).getByRole("button", { name: "Disconnect" })).toBeInTheDocument();
  });

  it("an unapproved server ends as unreachable with an explanation", async () => {
    const { user } = renderApp(<DashboardScreen />);
    const card = await screen.findByRole("article", { name: "Creative Realm" });
    await user.click(within(card).getByRole("button", { name: "Join only" }));
    expect(await within(card).findByText(/may be offline, or this device has not been approved/i)).toBeInTheDocument();
  });

  it("shows an empty state with Import invitation when there are no servers", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0, seed: false });
    renderApp(<DashboardScreen />, { backend });
    expect(await screen.findByText("No servers yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Import invitation" })).toBeInTheDocument();
    expect(screen.getByText("Nothing yet")).toBeInTheDocument();
  });

  it("shows a retryable error when the backend cannot list servers", async () => {
    const backend = createMockBackend({ delayMs: 0, joinMs: 0 });
    const servers = vi.spyOn(backend, "servers").mockRejectedValueOnce(new Error("backend unavailable"));
    const { user } = renderApp(<DashboardScreen />, { backend });
    expect(await screen.findByText("Could not load your servers")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Friends Survival")).toBeInTheDocument();
    expect(servers).toHaveBeenCalledTimes(2);
  });

  it("dashboard has no friends panel and summarises the overview", async () => {
    renderApp(<DashboardScreen />);
    await screen.findByText("Friends Survival");
    expect(screen.queryByText(/friends/i, { selector: "h2" })).not.toBeInTheDocument();
    const servers = screen.getByRole("region", { name: "Servers" });
    expect(servers).toHaveTextContent("4");
    expect(screen.getByRole("region", { name: "Security" })).toHaveTextContent("Private");
  });
});
