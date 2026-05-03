"use client";

import { createContext, useContext } from "react";

import type { ServerDetail } from "./api";

export const ServerDetailContext = createContext<ServerDetail | null>(null);

export function useServerDetailCtx(): ServerDetail {
	const v = useContext(ServerDetailContext);
	if (!v) {
		throw new Error(
			"useServerDetailCtx must be used inside the layout provider",
		);
	}
	return v;
}
