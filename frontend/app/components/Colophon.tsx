// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactElement } from "react";

/**
 * Canonical source for THIS build.
 *
 * AGPL-3.0-or-later section 13 requires that anyone interacting with this
 * program over a network be offered its Corresponding Source. That is the point
 * of this component: the offer travels with the binary instead of living in a
 * README nobody reaches from a hosted panel.
 *
 * If you modify anvil and host it, section 13 obliges you to point this at YOUR
 * modified source. Leaving it aimed here while shipping changes is a licence
 * violation, not a courtesy problem.
 */
const SOURCE_URL = "https://gitlab.cherkaoui.ch/HadiCherkaoui/anvil";
const LICENSE_URL = `${SOURCE_URL}/-/blob/main/LICENSE`;

/**
 * Bottom chassis rail: maker's mark, licence, and the source offer.
 *
 * Structural rhyme with CommandBar (same bg, same soft border, same px-5), but
 * deliberately shorter — this closes the instrument, it does not compete with
 * the command surface for height on a dense operations panel.
 *
 * Colour choices are contrast-led, not vibe-led: text-dim (3.74:1) and
 * text-faint (2.44:1) both fail WCAG AA for text this size against --color-bg,
 * so the prose sits on text-muted (5.78:1) and the mark on accent (7.45:1).
 */
export function Colophon(): ReactElement {
	return (
		<footer className="flex flex-wrap items-center justify-start gap-x-4 gap-y-2 border-t border-border-soft bg-bg px-5 py-2.5 font-mono text-[11px] text-text-muted sm:justify-between">
			<div className="flex flex-wrap items-center gap-x-2 gap-y-1">
				{/* Same glyph as the CommandBar brand mark — one mark, two places. */}
				<svg
					viewBox="0 0 24 24"
					width="14"
					height="14"
					fill="none"
					stroke="currentColor"
					strokeWidth={1.5}
					className="shrink-0 text-accent"
					aria-hidden="true"
				>
					<path d="M4 14l4-8h8l4 8M6 14h12v4H6z" />
				</svg>
				<span className="text-text-body">anvil</span>
				<span className="text-text-faint" aria-hidden="true">
					/
				</span>
				{/* The authorship claim, carried at the same weight as the project
				    name. This is the line that has to survive an unmodified rehost,
				    so it does not sit a tone quieter than everything around it. */}
				<span className="text-text-body">
					<span aria-hidden="true">© </span>Hadi Cherkaoui
				</span>
				<span className="text-text-faint" aria-hidden="true">
					/
				</span>
				<a
					href={LICENSE_URL}
					target="_blank"
					rel="noreferrer"
					className="rounded-sm transition-colors hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
				>
					AGPL-3.0-or-later
				</a>
			</div>

			<a
				href={SOURCE_URL}
				target="_blank"
				rel="noreferrer"
				className="rounded-sm border border-border px-2 py-0.5 text-text-muted transition-colors hover:border-accent hover:text-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			>
				Source
				<span aria-hidden="true"> ↗</span>
				<span className="sr-only"> (opens in a new tab)</span>
			</a>
		</footer>
	);
}
