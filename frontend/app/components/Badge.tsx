import type { CSSProperties, ReactElement } from "react";

import { cn } from "../lib/cn";

export type BadgeVariant =
	| "running"
	| "stopped"
	| "starting"
	| "stopping"
	| "error"
	| "update";

interface BadgeProps {
	variant: BadgeVariant;
	label?: string;
}

const LABELS: Record<BadgeVariant, string> = {
	running: "running",
	stopped: "stopped",
	starting: "starting",
	stopping: "stopping",
	error: "error",
	update: "update available",
};

const DOT_COLOR: Record<BadgeVariant, string> = {
	running: "bg-state-running",
	stopped: "bg-text-faint",
	starting: "bg-state-warning animate-pulse",
	stopping: "bg-state-warning animate-pulse",
	error: "bg-state-error",
	update: "bg-accent",
};

const TEXT_COLOR: Record<BadgeVariant, string> = {
	running: "text-text-body",
	stopped: "text-text-muted",
	starting: "text-text-body",
	stopping: "text-text-body",
	error: "text-state-error",
	update: "text-accent",
};

const RUNNING_GLOW: CSSProperties = {
	boxShadow: "0 0 8px var(--color-state-running-glow)",
};

export function Badge({ variant, label }: BadgeProps): ReactElement {
	return (
		<span
			className={cn(
				"inline-flex items-center gap-1.5 font-mono text-[11px]",
				TEXT_COLOR[variant],
			)}
		>
			<span
				className={cn("h-1.5 w-1.5 rounded-full", DOT_COLOR[variant])}
				style={variant === "running" ? RUNNING_GLOW : undefined}
			/>
			{label ?? LABELS[variant]}
		</span>
	);
}
