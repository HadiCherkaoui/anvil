"use client";

import { useEffect, useState } from "react";

import { fetchMcVersions, type McVersionsResponse } from "./api";

let cached: McVersionsResponse | undefined;
let inflight: Promise<McVersionsResponse> | undefined;

export function useMcVersions(): McVersionsResponse | undefined {
	const [value, setValue] = useState<McVersionsResponse | undefined>(cached);

	useEffect(() => {
		if (value !== undefined) return;
		if (inflight === undefined) {
			inflight = fetchMcVersions().then((v) => {
				cached = v;
				return v;
			});
		}
		let alive = true;
		inflight
			.then((v) => {
				if (alive) setValue(v);
			})
			.catch(() => {
				// Best-effort — leave value undefined; callers fall back to a
				// minimal hardcoded list (the create / settings pages do this).
			});
		return () => {
			alive = false;
		};
	}, [value]);

	return value;
}
