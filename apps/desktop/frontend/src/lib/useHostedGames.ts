import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "./backend";
import type { HostedGame } from "./types";

export function hostingError(error: unknown) {
  if (typeof error === "string") return error;
  return error instanceof Error ? error.message : "The hosting request failed";
}

export function useHostedGames(backend: Backend) {
  const [games, setGames] = useState<HostedGame[]>();
  const [error, setError] = useState("");
  const request = useRef(0);
  const invalidate = useCallback(() => { ++request.current; }, []);

  const refresh = useCallback(async () => {
    const current = ++request.current;
    try {
      const hosted = await backend.hostedGames();
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
  }, [backend]);

  useEffect(() => {
    void refresh();
    const interval = window.setInterval(() => void refresh(), 2000);
    return () => {
      invalidate();
      window.clearInterval(interval);
    };
  }, [invalidate, refresh]);

  return { games, error, refresh };
}
