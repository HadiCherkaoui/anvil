"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactElement } from "react";

const ROOT_LABEL = "anvil";

function decodeSegment(s: string): string {
	try {
		return decodeURIComponent(s);
	} catch {
		return s;
	}
}

export function PathBreadcrumb(): ReactElement {
	const pathname = usePathname();
	const segments = pathname.split("/").filter((s) => s.length > 0);
	const crumbs = [
		{ label: ROOT_LABEL, href: "/" },
		...segments.map((s, i) => ({
			label: decodeSegment(s),
			href: "/" + segments.slice(0, i + 1).join("/"),
		})),
	];
	return (
		<nav
			aria-label="breadcrumb"
			className="flex items-center gap-2 font-mono text-[12px] text-text-muted"
		>
			<svg
				viewBox="0 0 24 24"
				width="16"
				height="16"
				fill="none"
				stroke="currentColor"
				strokeWidth={1.5}
				className="text-accent"
				aria-hidden="true"
			>
				<path d="M4 14l4-8h8l4 8M6 14h12v4H6z" />
			</svg>
			{crumbs.map((c, i) => {
				const last = i === crumbs.length - 1;
				return (
					<span key={c.href} className="flex items-center gap-2">
						{i > 0 && <span className="text-text-faint">/</span>}
						{last ? (
							<span className="text-text-primary">{c.label}</span>
						) : (
							<Link
								href={c.href}
								className="rounded-sm transition-colors hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
							>
								{c.label}
							</Link>
						)}
					</span>
				);
			})}
		</nav>
	);
}
