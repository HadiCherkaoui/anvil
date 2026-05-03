// Hook for /api/servers/:id/update/stream — typed frames, reconnect with
// exponential backoff. Mirrors useLogsStream's onopen/hello semantics so
// the two WS hooks share lifecycle shape (audit §6.2).

"use client";

import { useEffect, useState } from "react";
import { z } from "zod";

const phaseSchema = z.enum([
	"queued",
	"announcing",
	"stopping",
	"backing-up",
	"swapping",
	"starting",
	"verifying",
	"succeeded",
	"rolling-back",
	"rolled-back",
	"failed",
]);

const resultSchema = z.enum(["succeeded", "failed-rolled-back", "failed"]);

const frameSchema = z.discriminatedUnion("type", [
	z.object({ type: z.literal("hello"), phase: phaseSchema }),
	z.object({ type: z.literal("progress"), phase: phaseSchema }),
	z.object({ type: z.literal("done"), result: resultSchema }),
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
	status: UpdateStreamStatus;
	endedReason: string | null;
}

const INITIAL: UpdateStreamState = {
	phase: null,
	result: null,
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
						setState((s) => ({
							...s,
							result: frame.result,
							status: "ended",
						}));
						break;
					}
					case "end": {
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
				// If the FSM ended cleanly we don't reconnect — `done`/`end`
				// is the terminal event for an update.
				setState((s) => {
					if (s.status === "ended") return s;
					scheduleReconnect();
					return s;
				});
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
