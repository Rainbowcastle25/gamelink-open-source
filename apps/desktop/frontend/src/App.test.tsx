import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import App from "./App";
import { Modal } from "./components/Modal";
import { createMockBackend } from "./lib/mockBackend";

describe("App", () => {
  it("shows the MOCK BACKEND ribbon only when the mock is active", async () => {
    const { unmount } = render(<App backend={createMockBackend({ delayMs: 0 })} mock />);
    expect(screen.getByText("MOCK BACKEND")).toBeInTheDocument();
    unmount();
    render(<App backend={createMockBackend({ delayMs: 0 })} mock={false} />);
    expect(screen.queryByText("MOCK BACKEND")).not.toBeInTheDocument();
  });

  it("navigates between every screen with landmarks in place", async () => {
    const user = userEvent.setup();
    render(<App backend={createMockBackend({ delayMs: 0, joinMs: 0 })} mock={false} />);
    expect(screen.getByRole("navigation", { name: "Main" })).toBeInTheDocument();
    expect(screen.getByRole("main")).toBeInTheDocument();
    expect(await screen.findByText("Private game servers for you and your friends")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Dashboard" })).toHaveAttribute("aria-current", "page");

    await user.click(screen.getByRole("button", { name: "Servers" }));
    expect(screen.getByRole("heading", { level: 1, name: "Servers" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Requests" }));
    expect(screen.getByRole("heading", { level: 1, name: "Requests" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Friends" }));
    expect(screen.getByText("Friends are coming later")).toBeInTheDocument();
    expect(screen.getAllByText("Planned for a future release")).toHaveLength(3);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("heading", { level: 1, name: "Settings" })).toBeInTheDocument();
  });

  it("Requests -> Import invitation opens the import dialog on the Servers screen", async () => {
    const user = userEvent.setup();
    render(<App backend={createMockBackend({ delayMs: 0, joinMs: 0 })} mock={false} />);
    await user.click(screen.getByRole("button", { name: "Requests" }));
    await user.click(await screen.findByRole("button", { name: "Import invitation" }));
    expect(screen.getByRole("heading", { level: 1, name: "Servers" })).toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: "Import invitation" })).toBeInTheDocument();
  });
});

describe("Modal", () => {
  it("traps focus, closes with Escape and restores focus", async () => {
    const user = userEvent.setup();
    let closed = 0;
    const { rerender } = render(
      <>
        <button>opener</button>
      </>,
    );
    const opener = screen.getByRole("button", { name: "opener" });
    opener.focus();
    rerender(
      <>
        <button>opener</button>
        <Modal title="Hello" onClose={() => closed++} footer={<button>Last</button>}>
          <input aria-label="first field" />
        </Modal>
      </>,
    );
    const dialog = screen.getByRole("dialog", { name: "Hello" });
    expect(dialog).toHaveAttribute("aria-modal", "true");
    // Focus starts inside the dialog and wraps from the last control to the first.
    expect(dialog.contains(document.activeElement)).toBe(true);
    await user.tab();
    await user.tab();
    await user.tab();
    expect(dialog.contains(document.activeElement)).toBe(true);
    await user.keyboard("{Escape}");
    expect(closed).toBe(1);
    rerender(
      <>
        <button>opener</button>
      </>,
    );
    expect(opener).toHaveFocus();
  });
});
