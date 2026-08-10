// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

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
				// Reset the shared promise so a later mount retries instead of
				// re-attaching to this rejected one for the rest of the session
				// (which left the version dropdown permanently empty). Callers
				// fall back to a minimal hardcoded list until a retry succeeds.
				inflight = undefined;
			});
		return () => {
			alive = false;
		};
	}, [value]);

	return value;
}
