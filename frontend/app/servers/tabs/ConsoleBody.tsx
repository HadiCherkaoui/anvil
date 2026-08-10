// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import type { ReactElement } from "react";

import { LiveLogPanel } from "../../components/LiveLogPanel";
import { RconCommand } from "../../components/RconCommand";
import { useServerDetailCtx } from "../../lib/server-detail-context";

export function ConsoleBody(): ReactElement {
	const detail = useServerDetailCtx();
	const logsEnabled = detail.status !== "stopped";
	// Keying on (id, enabled) forces a fresh hook instance when either
	// changes — cleaner than setState-in-effect resets, and means a
	// stop→start cycle starts the log buffer from empty rather than
	// re-using stale lines from the previous pod.
	const logsKey = `${detail.id}-${logsEnabled.toString()}`;
	return (
		<div className="flex flex-col gap-4">
			<LiveLogPanel key={logsKey} serverId={detail.id} enabled={logsEnabled} />
			<RconCommand
				serverId={detail.id}
				disabled={detail.status !== "running"}
			/>
		</div>
	);
}
