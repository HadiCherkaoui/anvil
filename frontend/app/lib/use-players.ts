"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import { ApiError, fetchPlayers, type PlayersResponse } from "./api";

const POLL_INTERVAL_MS = 10_000;

export type PlayersStatus = "loading" | "live" | "stale" | "error" | "stopped";

export interface UsePlayersResult {
	readonly data: PlayersResponse | null;
	readonly status: PlayersStatus;
	readonly lastError: string | null;
	readonly refresh: () => void;
}

/// Subscribes to the bulk-read endpoint with a 10 s poll, paused while
/// the document is hidden. Returns the latest snapshot, the connection
/// status, and a `refresh()` callback for out-of-band fetches (e.g.
/// after a successful action).
export function usePlayers(
	serverId: string,
	opts: { enabled: boolean },
): UsePlayersResult {
	const { enabled } = opts;
	const [data, setData] = useState<PlayersResponse | null>(null);
	const [status, setStatus] = useState<PlayersStatus>("loading");
	const [lastError, setLastError] = useState<string | null>(null);
	const dataRef = useRef<PlayersResponse | null>(null);
	const doFetchRef = useRef<(() => void) | null>(null);

	// Keep dataRef in sync so the polling effect can read the latest value
	// without depending on `data` (which would restart the polling interval).
	useEffect(() => {
		dataRef.current = data;
	}, [data]);

	const refresh = useCallback((): void => {
		doFetchRef.current?.();
	}, []);

	useEffect(() => {
		if (!enabled) return;
		let cancelled = false;
		let interval: number | null = null;
		let abort: AbortController | null = null;

		const doFetch = async (): Promise<void> => {
			abort?.abort();
			abort = new AbortController();
			try {
				const fresh = await fetchPlayers(serverId, abort.signal);
				if (cancelled) return;
				setData(fresh);
				setStatus("live");
				setLastError(null);
			} catch (err: unknown) {
				if (cancelled) return;
				if (
					err instanceof DOMException &&
					(err.name === "AbortError" || err.name === "TimeoutError")
				) {
					return;
				}
				if (err instanceof ApiError && err.code === "server_not_running") {
					setStatus("stopped");
					setData(null);
					setLastError(null);
					return;
				}
				setStatus(dataRef.current === null ? "error" : "stale");
				setLastError(
					err instanceof Error ? err.message : "unknown players-fetch error",
				);
			}
		};

		doFetchRef.current = (): void => {
			void doFetch();
		};

		const start = (): void => {
			void doFetch();
			interval = window.setInterval(() => {
				if (document.visibilityState === "visible") {
					void doFetch();
				}
			}, POLL_INTERVAL_MS);
		};

		const stop = (): void => {
			if (interval !== null) {
				window.clearInterval(interval);
				interval = null;
			}
			abort?.abort();
		};

		const onVisibilityChange = (): void => {
			if (document.visibilityState === "visible") {
				void doFetch();
			}
		};

		start();
		document.addEventListener("visibilitychange", onVisibilityChange);

		return (): void => {
			cancelled = true;
			doFetchRef.current = null;
			document.removeEventListener("visibilitychange", onVisibilityChange);
			stop();
		};
		// The effect restarts only on (serverId, enabled) changes. The polling
		// interval and visibility listener own their own re-fetch cadence via
		// doFetch. `data` is intentionally absent from deps — it is read through
		// `dataRef` to avoid restarting the interval on every successful fetch.
	}, [serverId, enabled]);

	// When disabled, derive the stopped snapshot at render time rather than
	// resetting state inside an effect (avoids react-hooks/set-state-in-effect).
	if (!enabled) {
		return { data: null, status: "stopped", lastError: null, refresh };
	}

	return { data, status, lastError, refresh };
}
