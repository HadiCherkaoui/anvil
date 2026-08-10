// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useEffect, useState } from "react";

import { fetchPaperVersions, type PaperVersionsResponse } from "./api";

let cached: PaperVersionsResponse | undefined;
let inflight: Promise<PaperVersionsResponse> | undefined;

/// Returns the Paper-supported MC version list. `undefined` while the
/// initial fetch is in flight; subsequent renders return the cached value.
export function usePaperVersions(): PaperVersionsResponse | undefined {
	const [value, setValue] = useState<PaperVersionsResponse | undefined>(cached);

	useEffect(() => {
		if (value !== undefined) return;
		if (inflight === undefined) {
			inflight = fetchPaperVersions().then((v) => {
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
				// Best-effort — leave value undefined so the caller can fall
				// back to the generic mc-versions list.
			});
		return () => {
			alive = false;
		};
	}, [value]);

	return value;
}
