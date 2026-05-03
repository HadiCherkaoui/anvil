"use client";

import type { ReactElement } from "react";

import { Card } from "../../components/Card";

export function PlayersBody(): ReactElement {
	return (
		<Card header="players">
			<p className="font-mono text-[12px] text-text-muted">
				player management arrives in v2.2.
			</p>
		</Card>
	);
}
