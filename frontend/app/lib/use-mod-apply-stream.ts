"use client";

import { useEffect, useRef, useState } from "react";

import type { UpdatePhase } from "./update-stream";

export type ApplyTarget = "mods" | "plugins";

export interface ModApplyStream {
	status: "connecting" | "open" | "reconnecting" | "closed";
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

interface ApplyFrame {
	type: string;
	phase?: UpdatePhase;
	result?: "succeeded" | "failed";
	reason?: string;
}

export function useModApplyStream(
	serverId: string | null,
	target: ApplyTarget = "mods",
): ModApplyStream {
	const [state, setState] = useState<ModApplyStream>(INITIAL);
	const cancelled = useRef(false);

	useEffect(() => {
		if (serverId === null) return undefined;
		cancelled.current = false;
		const url = `${
			window.location.protocol === "https:" ? "wss:" : "ws:"
		}//${window.location.host}/api/servers/${encodeURIComponent(
			serverId,
		)}/${target}/apply/stream`;
		let socket: WebSocket | null = null;
		let backoff = 1_000;
		const connect = (): void => {
			if (cancelled.current) return;
			socket = new WebSocket(url);
			socket.onopen = (): void => {
				setState((s) => ({ ...s, status: "open" }));
			};
			socket.onmessage = (ev): void => {
				try {
					const raw: unknown = JSON.parse(String(ev.data));
					if (typeof raw === "object" && raw !== null && "type" in raw) {
						const f = raw as ApplyFrame;
						if (f.type === "hello" || f.type === "progress") {
							setState((s) => ({ ...s, phase: f.phase ?? s.phase }));
						} else if (f.type === "done") {
							setState((s) => ({ ...s, result: f.result ?? null }));
						} else if (f.type === "end") {
							setState((s) => ({ ...s, endedReason: f.reason ?? null }));
						}
					}
				} catch {
					// ignore malformed frames
				}
			};
			socket.onerror = (): void => {
				if (cancelled.current) return;
				setState((s) => ({ ...s, status: "reconnecting" }));
			};
			socket.onclose = (): void => {
				if (cancelled.current) return;
				setState((s) => ({ ...s, status: "reconnecting" }));
				window.setTimeout(connect, backoff);
				backoff = Math.min(backoff * 2, 30_000);
			};
		};
		connect();
		return () => {
			cancelled.current = true;
			socket?.close();
		};
	}, [serverId, target]);

	return state;
}
