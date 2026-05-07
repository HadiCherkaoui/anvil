"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import {
	ApiError,
	fetchFileList,
	type FileListResponse,
	type ServerDetail,
} from "./api";

export type UseFilesStatus = "loading" | "warming" | "ready" | "error";

export interface UseFilesResult {
	readonly data: FileListResponse | null;
	readonly status: UseFilesStatus;
	readonly lastError: string | null;
	readonly refresh: () => void;
}

/**
 * Re-fetches on `(serverId, path)` change and on `refresh()`. No
 * polling — file lists only change as a result of the same anvil
 * endpoints, so the hook trusts post-action callbacks to nudge it.
 *
 * `status === "warming"` covers the helper-Pod boot path on the very
 * first request when the server is stopped (5–15 s). Subsequent fetches
 * use plain `loading`.
 */
export function useFiles(
	serverId: string,
	path: string,
	opts: { enabled: boolean; serverStatus: ServerDetail["status"] },
): UseFilesResult {
	const [data, setData] = useState<FileListResponse | null>(null);
	const [status, setStatus] = useState<UseFilesStatus>("loading");
	const [lastError, setLastError] = useState<string | null>(null);
	const abortRef = useRef<AbortController | null>(null);
	const firstFetchRef = useRef(true);
	const tickRef = useRef(0);

	const doFetch = useCallback(
		async (warming: boolean): Promise<void> => {
			abortRef.current?.abort();
			const ctrl = new AbortController();
			abortRef.current = ctrl;
			// If the controller is already aborted (effect torn down between
			// scheduling and running), don't dirty React state.
			if (ctrl.signal.aborted) return;
			setStatus(warming ? "warming" : "loading");
			tickRef.current += 1;
			const myTick = tickRef.current;
			try {
				const result = await fetchFileList(serverId, path, ctrl.signal);
				if (tickRef.current !== myTick) return;
				setData(result);
				setStatus("ready");
				setLastError(null);
				firstFetchRef.current = false;
			} catch (err: unknown) {
				if (err instanceof DOMException && err.name === "AbortError") return;
				if (tickRef.current !== myTick) return;
				const message =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				setStatus("error");
				setLastError(message);
			}
		},
		[serverId, path],
	);

	useEffect(() => {
		if (!opts.enabled) {
			abortRef.current?.abort();
			return undefined;
		}
		const warming = firstFetchRef.current && opts.serverStatus === "stopped";
		void doFetch(warming);
		return () => {
			abortRef.current?.abort();
		};
	}, [doFetch, opts.enabled, opts.serverStatus]);

	const refresh = useCallback((): void => {
		void doFetch(false);
	}, [doFetch]);

	return { data, status, lastError, refresh };
}
