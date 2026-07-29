// Live WebSocket client for /api/servers/{id}/logs/stream; reconnects with exponential backoff.

import { useEffect, useState } from "react";
import { z } from "zod";

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

const ANSI_ESCAPE = /\x1b\[[0-9;]*[A-Za-z]/g;

export function stripAnsi(line: string): string {
	return line.replace(ANSI_ESCAPE, "");
}

// `Caused by:` sits outside the \b(?:…)\b group on purpose. A trailing \b
// after the literal colon demands a *word* char next, but stack traces read
// `Caused by: java.lang.…` — space follows, so inside the group that
// alternative could never match and continuation lines were scored "info".
const ERROR_PATTERN = /\b(?:ERROR|FATAL|SEVERE|Exception)\b|Caused by:/;

export function classifyLine(line: string): LogLevel {
	if (ERROR_PATTERN.test(line)) return "error";
	if (/\bWARN(?:ING)?\b/.test(line)) return "warn";
	return "info";
}

// Lines from MC's RCON Listener / Client threads — fires `Thread … started` and
// `… shutting down` for every panel command. Drowns out signal in the live
// panel; full fidelity is still in `kubectl logs`.
//
// Match the bracketed thread tag itself (no trailing colon): the
// vanilla format is `[20:56:02] [RCON Listener #1/INFO] [...]: …`,
// so the colon lives after the next bracket pair, not this one.
const RCON_THREAD = /\[RCON (?:Listener|Client) [^\]]*\]/;

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

const DEFAULT_MAX_LINES = 2000;
const BACKOFF_INITIAL_MS = 1_000;
const BACKOFF_CAP_MS = 30_000;
const NORMAL_CLOSE_CODE = 1000;

export type UseLogsStreamOptions = {
	readonly maxLines?: number;
	readonly enabled?: boolean;
};

/// Subscribes to /api/servers/{id}/logs/stream and exposes a bounded
/// in-memory log tail. Reconnects with exponential backoff on
/// unexpected close. Cleans up on unmount.
///
/// When `enabled` is false (e.g. the server is stopped — no pod, no
/// logs) the hook keeps the WebSocket closed and reports an idle
/// snapshot. This avoids the 60s `wait_for_running` + reconnect-backoff
/// loop that would otherwise spin forever against a missing pod.
export function useLogsStream(
	id: string,
	options: UseLogsStreamOptions = {},
): UseLogsStreamResult {
	const maxLines = options.maxLines ?? DEFAULT_MAX_LINES;
	const enabled = options.enabled ?? true;
	const [lines, setLines] = useState<readonly LogLine[]>([]);
	const [status, setStatus] = useState<ConnectionStatus>("connecting");
	const [lastError, setLastError] = useState<string | null>(null);
	const [endedReason, setEndedReason] = useState<EndReason | null>(null);

	useEffect(() => {
		if (!enabled) return;

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
	}, [id, enabled]);

	if (!enabled) {
		return { lines: [], status: "closed", lastError: null, endedReason: null };
	}
	return { lines, status, lastError, endedReason };
}
