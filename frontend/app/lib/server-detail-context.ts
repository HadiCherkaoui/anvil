"use client";

import { createContext, useContext } from "react";

import type { ServerDetail } from "./api";

export interface ServerDetailValue {
	detail: ServerDetail;
	refresh: () => void;
}

export const ServerDetailContext = createContext<ServerDetailValue | null>(
	null,
);

export function useServerDetail(): ServerDetailValue {
	const v = useContext(ServerDetailContext);
	if (!v) {
		throw new Error(
			"useServerDetail must be used inside the layout provider",
		);
	}
	return v;
}

export function useServerDetailCtx(): ServerDetail {
	return useServerDetail().detail;
}
