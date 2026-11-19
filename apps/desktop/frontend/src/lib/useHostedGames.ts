import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "./backend";
import type { HostGame, HostedGame } from "./types";

export const LEGACY_AGENT_UPDATE = "Update the GameLink agent on this node to manage access from the desktop app.";

export function hostingError(error: unknown) {
  if (typeof error === "string") return error;
  return error instanceof Error ? error.message : "The hosting request failed";
}

export function useHostedGames(backend: Backend) {
  const load = useCallback(async () => ({ games: await backend.hostedGames() }), [backend]);
  return useGameList<HostedGame>(load, 2000);
}

export function useLinkedGames(backend: Backend, pollMs?: number) {
  const load = useCallback(() => backend.hostStatus(), [backend]);
  return useGameList<HostGame>(load, pollMs);
}

type GameList<T> = { games: T[]; refreshing?: boolean };

function useGameList<T>(load: () => Promise<GameList<T>>, pollMs?: number) {
  const [list, setList] = useState<GameList<T>>();
  const [error, setError] = useState("");
  const request = useRef(0);
  const invalidate = useCallback(() => { ++request.current; }, []);

  const refresh = useCallback(async () => {
    const current = ++request.current;
    try {
      const result = await load();
      if (current === request.current) {
        setList(result);
        setError("");
      }
    } catch (e) {
      if (current === request.current) {
        setList(undefined);
        setError(hostingError(e));
      }
    }
  }, [load]);

  useEffect(() => {
    let stopped = false;
    let timeout: number | undefined;
    async function poll() {
      await refresh();
      if (!stopped && pollMs !== undefined) timeout = window.setTimeout(() => void poll(), pollMs);
    }
    void poll();
    return () => {
      stopped = true;
      invalidate();
      window.clearTimeout(timeout);
    };
  }, [invalidate, pollMs, refresh]);

  useEffect(() => {
    if (pollMs !== undefined || !list?.refreshing) return;
    const timeout = window.setTimeout(() => void refresh(), 5000);
    return () => window.clearTimeout(timeout);
  }, [list, pollMs, refresh]);

  return { games: list?.games, refreshing: list?.refreshing, error, refresh };
}
