// Hook for /api/servers/:id/update/stream — mirrors useLogsStream's
// shape (typed frames, exponential reconnect) but for update-progress
// frames. Keeps the WS-handling boilerplate close to the consumer.

"use client";

import { useEffect, useRef, useState } from "react";
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

export type UpdateStreamStatus = "connecting" | "open" | "closed";

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

const BACKOFF_INITIAL_MS = 1000;
const BACKOFF_MAX_MS = 30_000;

export function useUpdateStream(serverId: string | null): UpdateStreamState {
	const [state, setState] = useState<UpdateStreamState>(INITIAL);
	const cancelled = useRef(false);

	useEffect(() => {
		if (serverId === null) return;
		cancelled.current = false;
		let socket: WebSocket | null = null;
		let backoff = BACKOFF_INITIAL_MS;
		let timer: ReturnType<typeof setTimeout> | null = null;

		const connect = (): void => {
			const proto = window.location.protocol === "https:" ? "wss" : "ws";
			const url = `${proto}://${window.location.host}/api/servers/${encodeURIComponent(serverId)}/update/stream`;
			setState((s) => ({ ...s, status: "connecting" }));
			socket = new WebSocket(url);

			socket.onopen = (): void => {
				backoff = BACKOFF_INITIAL_MS;
				setState((s) => ({ ...s, status: "open" }));
			};

			socket.onmessage = (ev: MessageEvent<string>): void => {
				try {
					const raw: unknown = JSON.parse(ev.data);
					const frame = frameSchema.parse(raw);
					if (frame.type === "hello" || frame.type === "progress") {
						setState((s) => ({ ...s, phase: frame.phase }));
					} else if (frame.type === "done") {
						setState((s) => ({ ...s, result: frame.result, status: "closed" }));
					} else {
						// end
						setState((s) => ({
							...s,
							endedReason: frame.reason,
							status: "closed",
						}));
					}
				} catch {
					// Malformed frame — skip silently; backend bug, not user-visible.
				}
			};

			socket.onclose = (): void => {
				if (cancelled.current) return;
				setState((s) => ({ ...s, status: "closed" }));
				timer = setTimeout(connect, backoff);
				backoff = Math.min(backoff * 2, BACKOFF_MAX_MS);
			};
		};

		connect();

		return (): void => {
			cancelled.current = true;
			if (timer !== null) clearTimeout(timer);
			socket?.close(1000, "client-closed");
		};
	}, [serverId]);

	return state;
}
