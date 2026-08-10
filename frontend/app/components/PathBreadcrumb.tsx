// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import Link from "next/link";
import { usePathname, useSearchParams } from "next/navigation";
import { Suspense, type ReactElement } from "react";

const ROOT_LABEL = "anvil";

function decodeSegment(s: string): string {
	try {
		return decodeURIComponent(s);
	} catch {
		return s;
	}
}

interface Crumb {
	label: string;
	href: string;
}

function buildCrumbs(
	pathname: string,
	name: string | null,
	tab: string | null,
): Crumb[] {
	const segments = pathname.split("/").filter((s) => s.length > 0);
	const out: Crumb[] = [{ label: ROOT_LABEL, href: "/" }];
	for (let i = 0; i < segments.length; i += 1) {
		const seg = segments[i];
		if (seg === undefined) continue;
		out.push({
			label: decodeSegment(seg),
			href: "/" + segments.slice(0, i + 1).join("/"),
		});
	}
	if (pathname === "/servers" && name !== null) {
		out.push({
			label: name,
			href: `/servers?name=${encodeURIComponent(name)}`,
		});
		if (tab !== null && tab !== "overview") {
			out.push({
				label: tab,
				href: `/servers?name=${encodeURIComponent(name)}&tab=${encodeURIComponent(tab)}`,
			});
		}
	}
	return out;
}

function Inner(): ReactElement {
	const pathname = usePathname();
	const search = useSearchParams();
	const crumbs = buildCrumbs(pathname, search.get("name"), search.get("tab"));
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
					<span
						key={`${c.href}-${i.toString()}`}
						className="flex items-center gap-2"
					>
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

export function PathBreadcrumb(): ReactElement {
	return (
		<Suspense fallback={null}>
			<Inner />
		</Suspense>
	);
}
