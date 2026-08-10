// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactElement, ReactNode } from "react";

import { cn } from "../lib/cn";

interface TooltipProps {
	label: string;
	children: ReactNode;
	className?: string;
}

export function Tooltip({
	label,
	children,
	className,
}: TooltipProps): ReactElement {
	return (
		<span className={cn("group relative inline-flex", className)}>
			{children}
			<span
				role="tooltip"
				className="pointer-events-none absolute bottom-full left-1/2 mb-1 -translate-x-1/2 whitespace-nowrap rounded-sm border border-border bg-elevated px-2 py-1 font-mono text-[10px] uppercase tracking-wider text-text-body opacity-0 transition-opacity group-hover:opacity-100"
			>
				{label}
			</span>
		</span>
	);
}
