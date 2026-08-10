// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

// Hook for /api/servers/:id/update/stream — typed frames, reconnect with exponential backoff.

"use client";

import { useEffect, useState } from "react";
import { z } from "zod";

export const phaseSchema = z.enum([
	"queued",
	"announcing",
	"stopping",
	"backing-up",
	"swapping",
	"starting",
	"verifying",
	"succeeded",
	"restoring",
	"rolling-back",
	"rolled-back",
	"failed",
]);

const resultSchema = z.enum(["succeeded", "failed-rolled-back", "failed"]);

const frameSchema = z.discriminatedUnion("type", [
	z.object({ type: z.literal("hello"), phase: phaseSchema }),
	z.object({ type: z.literal("progress"), phase: phaseSchema }),
	z.object({
		type: z.literal("done"),
		result: resultSchema,
		// Backend omits the key when there's no error (skip_serializing_if),
		// never sends null — `.optional()` alone keeps the validator honest.
		error: z.string().optional(),
	}),
	z.object({ type: z.literal("end"), reason: z.string() }),
]);

export type UpdatePhase = z.infer<typeof phaseSchema>;
export type UpdateResult = z.infer<typeof resultSchema>;

export type UpdateStreamStatus =
	| "connecting"
	| "live"
	| "reconnecting"
	| "ended"
	| "closed";

export interface UpdateStreamState {
	phase: UpdatePhase | null;
	result: UpdateResult | null;
	error: string | null;
	status: UpdateStreamStatus;
	endedReason: string | null;
}

const INITIAL: UpdateStreamState = {
	phase: null,
	result: null,
	error: null,
	status: "connecting",
	endedReason: null,
};

const BACKOFF_INITIAL_MS = 1_000;
const BACKOFF_MAX_MS = 30_000;
const NORMAL_CLOSE_CODE = 1000;

function parseFrame(raw: string): z.infer<typeof frameSchema> | null {
	try {
		const json: unknown = JSON.parse(raw);
		const parsed = frameSchema.safeParse(json);
		return parsed.success ? parsed.data : null;
	} catch {
		return null;
	}
}

export function useUpdateStream(serverId: string | null): UpdateStreamState {
	const [state, setState] = useState<UpdateStreamState>(INITIAL);

	useEffect(() => {
		if (serverId === null) return undefined;
		let cancelled = false;
		let ws: WebSocket | null = null;
		let backoff = BACKOFF_INITIAL_MS;
		let reconnectTimer: number | null = null;
		let terminal = false;

		const url = new URL(
			`/api/servers/${encodeURIComponent(serverId)}/update/stream`,
			window.location.href,
		);
		url.protocol = url.protocol === "https:" ? "wss:" : "ws:";

		const scheduleReconnect = (): void => {
			if (cancelled) return;
			setState((s) => ({ ...s, status: "reconnecting" }));
			const delay = backoff;
			backoff = Math.min(backoff * 2, BACKOFF_MAX_MS);
			reconnectTimer = window.setTimeout(connect, delay);
		};

		const connect = (): void => {
			if (cancelled) return;
			setState((s) => ({ ...s, status: "connecting" }));
			ws = new WebSocket(url);

			ws.onmessage = (ev: MessageEvent<unknown>): void => {
				if (typeof ev.data !== "string") return;
				const frame = parseFrame(ev.data);
				if (frame === null) return;
				switch (frame.type) {
					case "hello": {
						backoff = BACKOFF_INITIAL_MS;
						setState((s) => ({
							...s,
							status: "live",
							phase: frame.phase,
							endedReason: null,
						}));
						break;
					}
					case "progress": {
						setState((s) => ({ ...s, phase: frame.phase }));
						break;
					}
					case "done": {
						terminal = true;
						setState((s) => ({
							...s,
							result: frame.result,
							error: frame.error ?? null,
							status: "ended",
						}));
						break;
					}
					case "end": {
						terminal = true;
						setState((s) => ({
							...s,
							endedReason: frame.reason,
							status: "ended",
						}));
						break;
					}
				}
			};

			ws.onclose = (): void => {
				ws = null;
				if (cancelled) return;
				// Decide outside the state updater so StrictMode's double-invocation
				// can't fire `scheduleReconnect` twice.
				if (terminal) return;
				scheduleReconnect();
			};
		};

		connect();

		return (): void => {
			cancelled = true;
			if (reconnectTimer !== null) {
				window.clearTimeout(reconnectTimer);
			}
			if (ws !== null) {
				ws.onmessage = null;
				ws.onclose = null;
				ws.close(NORMAL_CLOSE_CODE);
			}
			setState((s) => ({ ...s, status: "closed" }));
		};
	}, [serverId]);

	return state;
}
