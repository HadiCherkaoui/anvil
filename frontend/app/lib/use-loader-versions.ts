"use client";

import { useEffect, useState } from "react";

import { fetchLoaderVersions, type LoaderVersions } from "./api";

const cache = new Map<string, Promise<LoaderVersions>>();

/// Lazy-loads + memoises Forge / NeoForge loader versions per runtime.
/// Returns `null` when `runtime` is null or while the upstream fetch is in
/// flight; switching `runtime` does not trigger a synchronous reset (the
/// derived `byRuntime[runtime]` lookup yields `undefined` until the next
/// fetch resolves).
export function useLoaderVersions(
	runtime: "forge" | "neoforge" | null,
): LoaderVersions | null {
	const [byRuntime, setByRuntime] = useState<Record<string, LoaderVersions>>(
		{},
	);

	useEffect(() => {
		if (runtime === null) return undefined;
		if (byRuntime[runtime] !== undefined) return undefined;
		let pending = cache.get(runtime);
		if (pending === undefined) {
			pending = fetchLoaderVersions(runtime);
			cache.set(runtime, pending);
		}
		let alive = true;
		pending
			.then((r) => {
				if (alive) {
					setByRuntime((m) => ({ ...m, [runtime]: r }));
				}
			})
			.catch(() => {
				// Surface "loader list unavailable" inline in the consumer; here
				// we keep `byRuntime` empty so the picker degrades gracefully.
			});
		return () => {
			alive = false;
		};
	}, [runtime, byRuntime]);

	if (runtime === null) return null;
	return byRuntime[runtime] ?? null;
}
