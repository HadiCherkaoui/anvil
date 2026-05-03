"use client";

import type { ReactElement } from "react";

import { Card } from "../../components/Card";
import { useServerDetailCtx } from "../../lib/server-detail-context";

export function ModsBody(): ReactElement {
	const detail = useServerDetailCtx();
	return (
		<Card header="mods">
			<p className="font-mono text-[12px] text-text-muted">
				mod browsing arrives in v2.1.
			</p>
			{detail.source_kind !== "vanilla" && (
				<p className="mt-2 font-mono text-[12px] text-text-body">
					modpack identity · {detail.source_kind}
				</p>
			)}
		</Card>
	);
}
