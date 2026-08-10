// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import Link from "next/link";
import type { ReactElement } from "react";

import { cn } from "../lib/cn";

export interface Tab {
	id: string;
	label: string;
	href: string;
	count?: number;
	mark?: boolean;
}

interface TabsProps {
	tabs: ReadonlyArray<Tab>;
	activeId: string;
}

export function Tabs({ tabs, activeId }: TabsProps): ReactElement {
	return (
		<nav className="flex gap-6 border-b border-border-soft">
			{tabs.map((t) => {
				const active = t.id === activeId;
				return (
					<Link
						key={t.id}
						href={t.href}
						className={cn(
							"relative py-3 font-mono text-[12px] uppercase tracking-wider transition-colors",
							"focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent rounded-sm",
							active
								? "text-text-primary"
								: "text-text-muted hover:text-text-body",
						)}
					>
						{t.label}
						{typeof t.count === "number" && (
							<span className="ml-2 text-text-faint">({t.count})</span>
						)}
						{t.mark === true && (
							<span className="ml-2 inline-block h-1.5 w-1.5 rounded-full bg-state-warning" />
						)}
						{active && (
							<span className="absolute bottom-0 left-0 right-0 h-px bg-accent" />
						)}
					</Link>
				);
			})}
		</nav>
	);
}
