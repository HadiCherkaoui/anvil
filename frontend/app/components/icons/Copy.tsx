// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactElement } from "react";

export function CopyIcon(): ReactElement {
	return (
		<svg
			width="14"
			height="14"
			viewBox="0 0 16 16"
			fill="none"
			stroke="currentColor"
			strokeWidth="1.5"
			strokeLinecap="round"
			strokeLinejoin="round"
			aria-hidden="true"
		>
			<rect x="4" y="4" width="9" height="9" rx="1" />
			<path d="M3 11V4a1 1 0 0 1 1-1h7" />
		</svg>
	);
}
