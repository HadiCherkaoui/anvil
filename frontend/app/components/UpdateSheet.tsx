"use client";

import type { ReactElement } from "react";

import { useUpdateStream, type UpdatePhase } from "../lib/update-stream";
import { cn } from "../lib/cn";

import { Sheet } from "./Sheet";

const ORDER: ReadonlyArray<UpdatePhase> = [
	"queued",
	"announcing",
	"stopping",
	"backing-up",
	"restoring",
	"swapping",
	"starting",
	"verifying",
	"succeeded",
];

interface UpdateSheetProps {
	serverId: string | null;
	isOpen: boolean;
	onClose: () => void;
}

export function UpdateSheet({
	serverId,
	isOpen,
	onClose,
}: UpdateSheetProps): ReactElement {
	const stream = useUpdateStream(isOpen ? serverId : null);
	const activeIdx = stream.phase ? ORDER.indexOf(stream.phase) : -1;

	return (
		<Sheet isOpen={isOpen} onClose={onClose} title="update" width={640}>
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
								{p}
							</li>
						);
					})}
				</ol>

				{stream.phase === "rolling-back" || stream.phase === "rolled-back" ? (
					<p className="mt-4 font-mono text-[12px] text-state-warning">
						rolled back to previous version
					</p>
				) : null}

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
