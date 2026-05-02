// Live WebSocket client for /api/servers/{id}/logs/stream.
// Validates every frame with Zod, strips ANSI escapes, classifies
// log lines into info/warn/error, and reconnects with exponential
// backoff on unexpected close.

import { useEffect, useState } from "react";
import { z } from "zod";

// --- frame schemas (mirror backend src/ws.rs) ----------------------------

const helloFrameSchema = z.object({
	type: z.literal("hello"),
	pod: z.string(),
	attached_at: z.string(),
});

const logFrameSchema = z.object({
	type: z.literal("log"),
	line: z.string(),
});

const errorFrameSchema = z.object({
	type: z.literal("error"),
	code: z.string(),
	message: z.string(),
});

const endFrameSchema = z.object({
	type: z.literal("end"),
	reason: z.enum(["pod-unavailable", "client-closed", "server-shutdown"]),
});

export const frameSchema = z.discriminatedUnion("type", [
	helloFrameSchema,
	logFrameSchema,
	errorFrameSchema,
	endFrameSchema,
]);

export type Frame = z.infer<typeof frameSchema>;
export type EndReason = z.infer<typeof endFrameSchema>["reason"];

// --- public types --------------------------------------------------------

export type LogLevel = "info" | "warn" | "error";

export type LogLine = {
	readonly key: number;
	readonly level: LogLevel;
	readonly text: string;
};

export type ConnectionStatus =
	| "connecting"
	| "live"
	| "reconnecting"
	| "closed";

export type UseLogsStreamResult = {
	readonly lines: readonly LogLine[];
	readonly status: ConnectionStatus;
	readonly lastError: string | null;
	readonly endedReason: EndReason | null;
};

// --- helpers (exported for testing/reuse) --------------------------------

const ANSI_ESCAPE = /\x1b\[[0-9;]*[A-Za-z]/g;

export function stripAnsi(line: string): string {
	return line.replace(ANSI_ESCAPE, "");
}

export function classifyLine(line: string): LogLevel {
	if (/\b(?:ERROR|FATAL|SEVERE|Exception|Caused by:)\b/.test(line))
		return "error";
	if (/\bWARN(?:ING)?\b/.test(line)) return "warn";
	return "info";
}

// Lines from MC's RCON Listener / Client threads — fires `Thread … started` and
// `… shutting down` for every panel command. Drowns out signal in the live
// panel; full fidelity is still in `kubectl logs`.
const RCON_THREAD = /\[RCON (?:Listener|Client) [^\]]*\]:/;

export function isNoise(line: string): boolean {
	return RCON_THREAD.test(line);
}

/// Parses one inbound text frame. Returns null on schema failure so the
/// caller can decide whether to surface or silently drop.
export function parseFrame(raw: string): Frame | null {
	try {
		const json: unknown = JSON.parse(raw);
		const parsed = frameSchema.safeParse(json);
		return parsed.success ? parsed.data : null;
	} catch {
		return null;
	}
}

// --- hook ----------------------------------------------------------------

const DEFAULT_MAX_LINES = 2000;
const BACKOFF_INITIAL_MS = 1_000;
const BACKOFF_CAP_MS = 30_000;
const NORMAL_CLOSE_CODE = 1000;

export type UseLogsStreamOptions = {
	readonly maxLines?: number;
};

/// Subscribes to /api/servers/{id}/logs/stream and exposes a bounded
/// in-memory log tail. Reconnects with exponential backoff on
/// unexpected close. Cleans up on unmount.
export function useLogsStream(
	id: string,
	options: UseLogsStreamOptions = {},
): UseLogsStreamResult {
	const maxLines = options.maxLines ?? DEFAULT_MAX_LINES;
	const [lines, setLines] = useState<readonly LogLine[]>([]);
	const [status, setStatus] = useState<ConnectionStatus>("connecting");
	const [lastError, setLastError] = useState<string | null>(null);
	const [endedReason, setEndedReason] = useState<EndReason | null>(null);

	useEffect(() => {
		let cancelled = false;
		let ws: WebSocket | null = null;
		let backoff = BACKOFF_INITIAL_MS;
		let reconnectTimer: number | null = null;
		let nextKey = 0;

		const url = new URL(
			`/api/servers/${encodeURIComponent(id)}/logs/stream`,
			window.location.href,
		);
		url.protocol = url.protocol === "https:" ? "wss:" : "ws:";

		const append = (level: LogLevel, text: string): void => {
			setLines((prev): readonly LogLine[] => {
				const next: LogLine[] =
					prev.length >= maxLines
						? [...prev.slice(prev.length - maxLines + 1)]
						: [...prev];
				next.push({ key: nextKey, level, text });
				nextKey += 1;
				return next;
			});
		};

		const scheduleReconnect = (): void => {
			if (cancelled) return;
			setStatus("reconnecting");
			const delay = backoff;
			backoff = Math.min(backoff * 2, BACKOFF_CAP_MS);
			reconnectTimer = window.setTimeout(connect, delay);
		};

		const connect = (): void => {
			if (cancelled) return;
			setStatus("connecting");
			ws = new WebSocket(url);

			ws.onmessage = (ev: MessageEvent<unknown>): void => {
				if (typeof ev.data !== "string") return;
				const frame = parseFrame(ev.data);
				if (frame === null) return;
				switch (frame.type) {
					case "hello": {
						backoff = BACKOFF_INITIAL_MS;
						setStatus("live");
						setLastError(null);
						// Clear any stale end reason from a previous attach
						// — otherwise the status dot keeps showing the old
						// reason after a successful reconnect.
						setEndedReason(null);
						break;
					}
					case "log": {
						const text = stripAnsi(frame.line);
						if (isNoise(text)) break;
						append(classifyLine(text), text);
						break;
					}
					case "error": {
						setLastError(`${frame.code}: ${frame.message}`);
						break;
					}
					case "end": {
						setEndedReason(frame.reason);
						break;
					}
				}
			};

			ws.onerror = (): void => {
				setLastError("websocket error");
			};

			ws.onclose = (): void => {
				ws = null;
				if (cancelled) return;
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
				ws.onerror = null;
				ws.onclose = null;
				ws.close(NORMAL_CLOSE_CODE);
			}
			setStatus("closed");
		};
		// eslint-disable-next-line react-hooks/exhaustive-deps -- maxLines is read once at mount; changing it mid-life would orphan buffered lines anyway
	}, [id]);

	return { lines, status, lastError, endedReason };
}
