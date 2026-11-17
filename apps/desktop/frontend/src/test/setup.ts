import "@testing-library/jest-dom/vitest";
import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";
import { resetToasts } from "../lib/toast";

// jsdom has no clipboard; install a stub that tests can spy on.
if (!navigator.clipboard) {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: vi.fn().mockResolvedValue(undefined), readText: vi.fn().mockResolvedValue("") },
  });
}

afterEach(() => {
  cleanup();
  resetToasts();
  vi.restoreAllMocks();
});
