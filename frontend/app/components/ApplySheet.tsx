// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useEffect, type ReactElement } from "react";

import {
	useModApplyStream,
	type ApplyTarget,
} from "../lib/use-mod-apply-stream";
import type { UpdatePhase } from "../lib/update-stream";
import { cn } from "../lib/cn";

import { Sheet } from "./Sheet";

const AUTO_CLOSE_DELAY_MS = 2_000;
const AUTO_CLOSE_DELAY_FAILED_MS = 10_000;

// Sync FSM has no verify step — it doesn't boot the server. The server is
// only restarted when it was running before the apply (was_running pattern),
// so `starting` is shown but only lights up in that case.
const ORDER: ReadonlyArray<UpdatePhase> = [
	"queued",
	"stopping",
	"swapping",
	"starting",
	"succeeded",
];

function labels(target: ApplyTarget): Record<UpdatePhase, string> {
	return {
		queued: "queued",
		announcing: "announcing",
		stopping: "stopping",
		"backing-up": "backing up",
		swapping: target === "plugins" ? "syncing plugins" : "syncing mods",
		starting: "starting",
		verifying: "verifying",
		succeeded: "succeeded",
		restoring: "restoring",
		"rolling-back": "rolling back",
		"rolled-back": "rolled back",
		failed: "failed",
	};
}

interface Props {
	serverId: string | null;
	isOpen: boolean;
	onClose: () => void;
	target?: ApplyTarget;
}

export function ApplySheet({
	serverId,
	isOpen,
	onClose,
	target = "mods",
}: Props): ReactElement {
	const stream = useModApplyStream(isOpen ? serverId : null, target);
	const activeIdx = stream.phase ? ORDER.indexOf(stream.phase) : -1;
	const sheetTitle = target === "plugins" ? "apply plugins" : "apply mods";
	const labelMap = labels(target);

	useEffect(() => {
		if (!isOpen || stream.result === null) return undefined;
		const delay =
			stream.result === "succeeded"
				? AUTO_CLOSE_DELAY_MS
				: AUTO_CLOSE_DELAY_FAILED_MS;
		const t = window.setTimeout(onClose, delay);
		return () => {
			window.clearTimeout(t);
		};
	}, [isOpen, stream.result, onClose]);

	return (
		<Sheet isOpen={isOpen} onClose={onClose} title={sheetTitle} width={640}>
			<div className="p-5">
				<ol className="flex flex-col gap-2 font-mono text-[12px]">
					{ORDER.map((p, i) => {
						const reached = activeIdx >= 0 && i <= activeIdx;
						const active = stream.phase === p;
						return (
							<li
								key={p}
								className={cn(
									"flex items-center gap-3",
									reached ? "text-text-body" : "text-text-faint",
									active && "text-accent",
								)}
							>
								<span
									className={cn(
										"h-1.5 w-1.5 rounded-full",
										active
											? "bg-accent"
											: reached
												? "bg-state-running"
												: "bg-text-faint",
									)}
								/>
								{labelMap[p]}
							</li>
						);
					})}
				</ol>
				{stream.result !== null && (
					<p className="mt-4 font-mono text-[12px] text-text-body">
						result · {stream.result}
					</p>
				)}
				{stream.endedReason !== null && (
					<p className="mt-1 font-mono text-[12px] text-state-error">
						{stream.endedReason}
					</p>
				)}
				{stream.status === "reconnecting" && (
					<p className="mt-2 font-mono text-[11px] text-text-muted">
						reconnecting…
					</p>
				)}
			</div>
		</Sheet>
	);
}
