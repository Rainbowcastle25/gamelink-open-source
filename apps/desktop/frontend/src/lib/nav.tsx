import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import type { Screen } from "./types";

export interface NavOptions {
  /** Open the "Import invitation" dialog on the Servers screen. */
  openImport?: boolean;
  /** Open the "Add server manually" dialog on the Servers screen. */
  openAdd?: boolean;
}

export type NavIntent = "import" | "add" | null;

interface NavValue {
  screen: Screen;
  intent: NavIntent;
  go: (screen: Screen, opts?: NavOptions) => void;
  consumeIntent: () => void;
}

const NavContext = createContext<NavValue>({
  screen: "dashboard",
  intent: null,
  go: () => {},
  consumeIntent: () => {},
});

export function NavProvider({ children, initial = "dashboard" }: { children: ReactNode; initial?: Screen }) {
  const [screen, setScreen] = useState<Screen>(initial);
  const [intent, setIntent] = useState<NavIntent>(null);
  const go = useCallback((s: Screen, opts?: NavOptions) => {
    setScreen(s);
    setIntent(opts?.openImport ? "import" : opts?.openAdd ? "add" : null);
  }, []);
  const consumeIntent = useCallback(() => setIntent(null), []);
  const value = useMemo(() => ({ screen, intent, go, consumeIntent }), [screen, intent, go, consumeIntent]);
  return <NavContext.Provider value={value}>{children}</NavContext.Provider>;
}

export function useNav(): NavValue {
  return useContext(NavContext);
}
