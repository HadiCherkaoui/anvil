// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactElement, ReactNode } from "react";

import { cn } from "../lib/cn";

interface CardProps {
	header?: ReactNode;
	children: ReactNode;
	className?: string;
}

export function Card({ header, children, className }: CardProps): ReactElement {
	return (
		<div
			className={cn("rounded-md border border-border bg-surface", className)}
		>
			{header !== undefined && (
				<div className="border-b border-border-soft px-4 py-3 font-mono text-[11px] uppercase tracking-wider text-text-muted">
					{header}
				</div>
			)}
			<div className="p-4">{children}</div>
		</div>
	);
}
