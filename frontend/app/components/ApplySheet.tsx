"use client";

import type { ReactElement } from "react";

import {
	useModApplyStream,
	type ApplyTarget,
} from "../lib/use-mod-apply-stream";
import type { UpdatePhase } from "../lib/update-stream";
import { cn } from "../lib/cn";

import { Sheet } from "./Sheet";

const ORDER: ReadonlyArray<UpdatePhase> = [
	"queued",
	"stopping",
	"swapping",
	"starting",
	"verifying",
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
