"use client";

import { useEffect, type ReactElement } from "react";

import { useUpdateStream, type UpdatePhase } from "../lib/update-stream";
import { cn } from "../lib/cn";

import { Sheet } from "./Sheet";

// How long the result line stays visible before the sheet auto-closes.
// Long enough to read "result · succeeded", short enough to feel quick.
const AUTO_CLOSE_DELAY_MS = 2_000;
// On a real failure we keep the sheet open longer so the user has time
// to read the error reason. They can still close manually.
const AUTO_CLOSE_DELAY_FAILED_MS = 10_000;

export type UpdateFlow = "update" | "backup" | "restore";

const UPDATE_ORDER: ReadonlyArray<UpdatePhase> = [
	"queued",
	"announcing",
	"stopping",
	"backing-up",
	"swapping",
	"starting",
	"verifying",
	"succeeded",
];

const BACKUP_ORDER: ReadonlyArray<UpdatePhase> = [
	"queued",
	"stopping",
	"backing-up",
	"starting",
	"succeeded",
];

const RESTORE_ORDER: ReadonlyArray<UpdatePhase> = [
	"queued",
	"announcing",
	"stopping",
	"restoring",
	"swapping",
	"starting",
	"verifying",
	"succeeded",
];

const ORDERS: Record<UpdateFlow, ReadonlyArray<UpdatePhase>> = {
	update: UPDATE_ORDER,
	backup: BACKUP_ORDER,
	restore: RESTORE_ORDER,
};

const TITLES: Record<UpdateFlow, string> = {
	update: "update",
	backup: "backup",
	restore: "restore",
};

interface UpdateSheetProps {
	serverId: string | null;
	isOpen: boolean;
	onClose: () => void;
	flow?: UpdateFlow;
}

export function UpdateSheet({
	serverId,
	isOpen,
	onClose,
	flow = "update",
}: UpdateSheetProps): ReactElement {
	const stream = useUpdateStream(isOpen ? serverId : null);
	const order = ORDERS[flow];
	const activeIdx = stream.phase ? order.indexOf(stream.phase) : -1;

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
		<Sheet isOpen={isOpen} onClose={onClose} title={TITLES[flow]} width={640}>
			<div className="p-5">
				<ol className="flex flex-col gap-2 font-mono text-[12px]">
					{order.map((p, i) => {
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

				{stream.error !== null && (
					<pre className="mt-2 whitespace-pre-wrap break-words rounded border border-state-error/40 bg-state-error/5 p-2 font-mono text-[11px] text-state-error">
						{stream.error}
					</pre>
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
