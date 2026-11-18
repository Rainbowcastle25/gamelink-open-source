import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "./backend";
import type { HostGame, HostedGame } from "./types";

export function hostingError(error: unknown) {
  if (typeof error === "string") return error;
  return error instanceof Error ? error.message : "The hosting request failed";
}

export function useHostedGames(backend: Backend) {
  const load = useCallback(() => backend.hostedGames(), [backend]);
  return useGameList<HostedGame>(load, 2000);
}

export function useLinkedGames(backend: Backend, pollMs?: number) {
  const load = useCallback(async () => (await backend.hostStatus()).games, [backend]);
  return useGameList<HostGame>(load, pollMs);
}

function useGameList<T>(load: () => Promise<T[]>, pollMs?: number) {
  const [games, setGames] = useState<T[]>();
  const [error, setError] = useState("");
  const request = useRef(0);
  const invalidate = useCallback(() => { ++request.current; }, []);

  const refresh = useCallback(async () => {
    const current = ++request.current;
    try {
      const hosted = await load();
      if (current === request.current) {
        setGames(hosted);
        setError("");
      }
    } catch (e) {
      if (current === request.current) {
        setGames(undefined);
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

  return { games, error, refresh };
}
