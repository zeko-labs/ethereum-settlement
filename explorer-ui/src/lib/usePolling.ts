import { useCallback, useEffect, useRef, useState } from "react";

export interface PollingState<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  updatedAt: number | null;
  refresh: () => void;
}

export function usePolling<T>(
  load: (signal: AbortSignal) => Promise<T>,
  intervalMs: number,
  key = "default",
): PollingState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [generation, setGeneration] = useState(0);
  const mounted = useRef(true);

  const refresh = useCallback(() => setGeneration((value) => value + 1), []);

  useEffect(() => {
    mounted.current = true;
    let controller: AbortController | null = null;
    const run = async () => {
      if (document.visibilityState === "hidden") return;
      controller?.abort();
      controller = new AbortController();
      setLoading((value) => data === null || value);
      try {
        const next = await load(controller.signal);
        if (!mounted.current) return;
        setData(next);
        setError(null);
        setUpdatedAt(Date.now());
      } catch (caught) {
        if (!controller.signal.aborted && mounted.current)
          setError(
            caught instanceof Error
              ? caught.message
              : "Unable to load explorer data",
          );
      } finally {
        if (mounted.current) setLoading(false);
      }
    };
    void run();
    const timer = window.setInterval(() => void run(), intervalMs);
    const visible = () => {
      if (document.visibilityState === "visible") void run();
    };
    document.addEventListener("visibilitychange", visible);
    return () => {
      mounted.current = false;
      controller?.abort();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", visible);
    };
    // `key` and `generation` intentionally restart the request lifecycle.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intervalMs, key, generation]);

  return { data, error, loading, updatedAt, refresh };
}
