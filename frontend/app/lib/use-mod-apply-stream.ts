"use client";

import { useEffect, useRef, useState } from "react";
import { z } from "zod";

import { phaseSchema, type UpdatePhase } from "./update-stream";

export type ApplyTarget = "mods" | "plugins";

export type ModApplyStatus =
	| "connecting"
	| "live"
	| "reconnecting"
	| "ended"
	| "closed";

export interface ModApplyStream {
	status: ModApplyStatus;
	phase: UpdatePhase | null;
	result: "succeeded" | "failed" | null;
	endedReason: string | null;
}

const INITIAL: ModApplyStream = {
	status: "connecting",
	phase: null,
	result: null,
	endedReason: null,
};

const resultSchema = z.enum(["succeeded", "failed"]);

const frameSchema = z.discriminatedUnion("type", [
	z.object({ type: z.literal("hello"), phase: phaseSchema.optional() }),
	z.object({ type: z.literal("progress"), phase: phaseSchema }),
	z.object({ type: z.literal("done"), result: resultSchema }),
	z.object({ type: z.literal("end"), reason: z.string() }),
]);

const BACKOFF_INITIAL_MS = 1_000;
const BACKOFF_MAX_MS = 30_000;
const NORMAL_CLOSE_CODE = 1000;

function parseFrame(raw: string): z.infer<typeof frameSchema> | null {
	try {
		const json: unknown = JSON.parse(raw);
		const parsed = frameSchema.safeParse(json);
		if (!parsed.success) {
			console.warn("use-mod-apply-stream: invalid frame", parsed.error);
			return null;
		}
		return parsed.data;
	} catch (err: unknown) {
		console.warn("use-mod-apply-stream: malformed JSON frame", err);
		return null;
	}
}

export function useModApplyStream(
	serverId: string | null,
	target: ApplyTarget = "mods",
): ModApplyStream {
	const [state, setState] = useState<ModApplyStream>(INITIAL);
	// Tracks whether the FSM has emitted a terminal frame (`done` or `end`).
	// The WS server closes the socket right after, and we must NOT reconnect
	// in that case — otherwise the stream re-attaches and replays forever.
	const terminalRef = useRef(false);

	useEffect(() => {
		if (serverId === null) return undefined;
		terminalRef.current = false;

		let cancelled = false;
		let ws: WebSocket | null = null;
		let backoff = BACKOFF_INITIAL_MS;
		let reconnectTimer: number | null = null;

		const url = new URL(
			`/api/servers/${encodeURIComponent(serverId)}/${target}/apply/stream`,
			window.location.href,
		);
		url.protocol = url.protocol === "https:" ? "wss:" : "ws:";

		const scheduleReconnect = (): void => {
			if (cancelled || terminalRef.current) return;
			setState((s) => ({ ...s, status: "reconnecting" }));
			const delay = backoff;
			backoff = Math.min(backoff * 2, BACKOFF_MAX_MS);
			reconnectTimer = window.setTimeout(connect, delay);
		};

		const connect = (): void => {
			if (cancelled || terminalRef.current) return;
			setState((s) => ({ ...s, status: "connecting" }));
			ws = new WebSocket(url);

			ws.onmessage = (ev: MessageEvent<unknown>): void => {
				if (typeof ev.data !== "string") return;
				const frame = parseFrame(ev.data);
				if (frame === null) return;
				switch (frame.type) {
					// "live" + backoff reset only on hello (not onopen) — a
					// socket that opens but never sends hello isn't attached;
					// matches useLogsStream / useUpdateStream semantics.
					case "hello": {
						backoff = BACKOFF_INITIAL_MS;
						setState((s) => ({
							...s,
							status: "live",
							phase: frame.phase ?? s.phase,
						}));
						break;
					}
					case "progress": {
						setState((s) => ({ ...s, phase: frame.phase }));
						break;
					}
					case "done": {
						terminalRef.current = true;
						setState((s) => ({
							...s,
							result: frame.result,
							status: "ended",
						}));
						break;
					}
					case "end": {
						terminalRef.current = true;
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
				if (cancelled || terminalRef.current) return;
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
				ws.onopen = null;
				ws.onmessage = null;
				ws.onclose = null;
				ws.close(NORMAL_CLOSE_CODE);
			}
		};
	}, [serverId, target]);

	return state;
}
