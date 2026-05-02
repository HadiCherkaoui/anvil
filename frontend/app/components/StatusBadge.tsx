"use client";

import type { ReactElement } from "react";

import type { ServerStatus } from "../lib/api";

interface StatusBadgeProps {
	status: ServerStatus;
}

const STATUS_CONFIG: Record<
	ServerStatus,
	{ label: string; pill: string; dot: string; pulsing: boolean }
> = {
	running: {
		label: "running",
		pill: "bg-green-500/15 text-green-300",
		dot: "bg-green-500",
		pulsing: false,
	},
	stopped: {
		label: "stopped",
		pill: "bg-slate-500/15 text-slate-300",
		dot: "bg-slate-500",
		pulsing: false,
	},
	starting: {
		label: "starting",
		pill: "bg-amber-500/15 text-amber-300",
		dot: "bg-amber-500",
		pulsing: true,
	},
	stopping: {
		label: "stopping",
		pill: "bg-amber-500/15 text-amber-300",
		dot: "bg-amber-500",
		pulsing: true,
	},
	error: {
		label: "error",
		pill: "bg-red-500/15 text-red-300",
		dot: "bg-red-500",
		pulsing: false,
	},
};

export function StatusBadge({ status }: StatusBadgeProps): ReactElement {
	const config = STATUS_CONFIG[status];
	const dotClass = `inline-block size-1.5 rounded-full ${config.dot}${
		config.pulsing ? " animate-pulse" : ""
	}`;
	return (
		<span
			className={`inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 font-mono text-xs ${config.pill}`}
		>
			<span className={dotClass} />
			{config.label}
		</span>
	);
}
