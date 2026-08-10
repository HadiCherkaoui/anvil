// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useEffect, useRef, useState, type ReactElement } from "react";

import {
	type ConnectionStatus,
	type LogLevel,
	useLogsStream,
} from "../lib/logs-stream";
import { friendlyEndReason } from "../lib/end-reason";
import { cn } from "../lib/cn";

interface LiveLogPanelProps {
	readonly serverId: string;
	readonly enabled: boolean;
}

const SCROLL_BOTTOM_THRESHOLD_PX = 50;

const LEVEL_CLASS: Record<LogLevel, string> = {
	info: "text-text-body",
	warn: "text-state-warning",
	error: "text-state-error",
};

const STATUS_DOT: Record<ConnectionStatus, string> = {
	connecting: "bg-text-muted",
	live: "bg-state-running",
	reconnecting: "bg-state-warning animate-pulse",
	closed: "bg-text-faint",
};

export function LiveLogPanel({
	serverId,
	enabled,
}: LiveLogPanelProps): ReactElement {
	const { lines, status, lastError, endedReason } = useLogsStream(serverId, {
		enabled,
	});
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

	const statusText = !enabled
		? "server stopped"
		: endedReason !== null
			? `ended · ${friendlyEndReason(endedReason)}`
			: status;
	const emptyText = !enabled
		? "(start the server to see logs)"
		: "(waiting for log lines…)";

	return (
		<section className="flex flex-col gap-3 rounded-md border border-border bg-surface p-4">
			<div className="flex items-baseline justify-between">
				<h2 className="font-mono text-[11px] uppercase tracking-wider text-text-muted">
					live logs
				</h2>
				<div className="flex items-center gap-3 font-mono text-[11px] text-text-muted">
					<span className="inline-flex items-center gap-1.5">
						<span
							className={cn("h-1.5 w-1.5 rounded-full", STATUS_DOT[status])}
						/>
						<span>{statusText}</span>
					</span>
					{!autoScroll && (
						<button
							type="button"
							onClick={jumpToLatest}
							className="text-text-muted transition-colors hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent rounded-sm"
						>
							jump to latest ↓
						</button>
					)}
				</div>
			</div>
			<div
				ref={containerRef}
				onScroll={onScroll}
				className="max-h-96 overflow-auto rounded-sm bg-bg p-3 font-mono text-[12px] leading-relaxed"
			>
				{lines.length === 0 ? (
					<span className="text-text-faint">{emptyText}</span>
				) : (
					lines.map((l) => (
						<div key={l.key} className={LEVEL_CLASS[l.level]}>
							{l.text}
						</div>
					))
				)}
			</div>
			{lastError !== null && (
				<p className="font-mono text-[11px] text-state-error">{lastError}</p>
			)}
		</section>
	);
}
