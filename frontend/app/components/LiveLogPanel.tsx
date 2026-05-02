"use client";

import { useEffect, useRef, useState, type ReactElement } from "react";

import {
	type ConnectionStatus,
	type LogLevel,
	useLogsStream,
} from "../lib/logs-stream";

interface LiveLogPanelProps {
	readonly serverId: string;
}

const SCROLL_BOTTOM_THRESHOLD_PX = 50;

const LEVEL_CLASS: Record<LogLevel, string> = {
	info: "text-slate-300",
	warn: "text-amber-300",
	error: "text-red-400",
};

export function LiveLogPanel({ serverId }: LiveLogPanelProps): ReactElement {
	const { lines, status, lastError, endedReason } = useLogsStream(serverId);
	const containerRef = useRef<HTMLDivElement | null>(null);
	const [autoScroll, setAutoScroll] = useState(true);

	useEffect(() => {
		if (!autoScroll) return;
		const el = containerRef.current;
		if (el === null) return;
		el.scrollTop = el.scrollHeight;
	}, [lines, autoScroll]);

	const onScroll = (): void => {
		const el = containerRef.current;
		if (el === null) return;
		const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
		setAutoScroll(distanceFromBottom <= SCROLL_BOTTOM_THRESHOLD_PX);
	};

	const jumpToLatest = (): void => {
		const el = containerRef.current;
		if (el === null) return;
		el.scrollTop = el.scrollHeight;
		setAutoScroll(true);
	};

	return (
		<section className="flex flex-col gap-3 rounded-lg border border-slate-800 p-4">
			<div className="flex items-baseline justify-between">
				<h2 className="text-xs uppercase tracking-wide text-slate-400">
					Live logs
				</h2>
				<div className="flex items-center gap-3 text-xs">
					<StatusDot status={status} endedReason={endedReason} />
					{!autoScroll && (
						<button
							type="button"
							onClick={jumpToLatest}
							className="text-xs text-slate-400 hover:text-slate-100"
						>
							jump to latest ↓
						</button>
					)}
				</div>
			</div>
			<div
				ref={containerRef}
				onScroll={onScroll}
				className="max-h-96 overflow-auto rounded-md bg-slate-950 p-3 font-mono text-xs leading-relaxed"
			>
				{lines.length === 0 ? (
					<span className="text-slate-500">(waiting for log lines…)</span>
				) : (
					lines.map((l) => (
						<div key={l.key} className={LEVEL_CLASS[l.level]}>
							{l.text}
						</div>
					))
				)}
			</div>
			{lastError !== null && (
				<p className="text-xs text-red-400">{lastError}</p>
			)}
		</section>
	);
}

interface StatusDotProps {
	readonly status: ConnectionStatus;
	readonly endedReason: string | null;
}

function StatusDot({ status, endedReason }: StatusDotProps): ReactElement {
	const labelMap: Record<ConnectionStatus, { dot: string; text: string }> = {
		connecting: { dot: "bg-amber-400", text: "connecting" },
		live: { dot: "bg-green-400", text: "live" },
		reconnecting: { dot: "bg-amber-400", text: "reconnecting" },
		closed: { dot: "bg-slate-500", text: "closed" },
	};
	const display = labelMap[status];
	const text = endedReason !== null ? `ended (${endedReason})` : display.text;
	return (
		<span className="inline-flex items-center gap-1.5 text-slate-400">
			<span className={`h-2 w-2 rounded-full ${display.dot}`} />
			<span>{text}</span>
		</span>
	);
}
