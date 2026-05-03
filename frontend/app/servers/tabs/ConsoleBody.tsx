"use client";

import type { ReactElement } from "react";

import { LiveLogPanel } from "../../components/LiveLogPanel";
import { RconCommand } from "../../components/RconCommand";
import { useServerDetailCtx } from "../../lib/server-detail-context";

export function ConsoleBody(): ReactElement {
	const detail = useServerDetailCtx();
	return (
		<div className="flex flex-col gap-4">
			<LiveLogPanel serverId={detail.id} />
			<RconCommand
				serverId={detail.id}
				disabled={detail.status !== "running"}
			/>
		</div>
	);
}
