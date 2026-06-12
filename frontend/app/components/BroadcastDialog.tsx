"use client";

import { useState, type ReactElement } from "react";

import { ApiError, broadcastMessage } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

interface BroadcastDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
}

const MSG_MAX = 256;
const CONTROL = /[\x00-\x1f\x7f]/;

export function BroadcastDialog({
	open,
	onClose,
	serverId,
}: BroadcastDialogProps): ReactElement {
	const [msg, setMsg] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const toast = useToast();

	const byteLen = new TextEncoder().encode(msg).byteLength;
	const tooLong = byteLen > MSG_MAX;
	const hasControl = CONTROL.test(msg);
	const valid = msg.length > 0 && !tooLong && !hasControl;

	const handleClose = (): void => {
		setMsg("");
		setError(null);
		onClose();
	};

	const onSubmit = (): void => {
		if (!valid) return;
		setError(null);
		setBusy(true);
		void broadcastMessage(serverId, msg)
			.then(() => {
				toast.push("broadcast sent", "success");
				setMsg("");
				onClose();
			})
			.catch((err: unknown) => {
				setError(
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error",
				);
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<Modal
			open={open}
			onClose={handleClose}
			title="broadcast — /say"
			maxWidth="md"
		>
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				<label className="flex flex-col gap-1.5">
					<span className="text-[11px] uppercase tracking-wider text-text-muted">
						message
					</span>
					<textarea
						value={msg}
						onChange={(e) => {
							// Strip newlines — broadcast endpoint rejects control chars; /say is single-line.
							setMsg(e.target.value.replace(/[\n\r]/g, ""));
						}}
						autoFocus
						rows={3}
						placeholder="restart in 5 minutes — please log out"
						className="w-full resize-none rounded-md border border-border bg-bg px-3 py-2 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
					/>
					<div className="flex justify-between text-[11px] text-text-dim">
						<span>broadcasts to all online players via /say</span>
						<span className={tooLong ? "text-state-error" : "text-text-dim"}>
							{byteLen} / {MSG_MAX}
						</span>
					</div>
					{hasControl && (
						<span className="text-[11px] text-state-error">
							no newlines or control chars
						</span>
					)}
				</label>
				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={handleClose} disabled={busy}>
						cancel
					</Button>
					<Button
						variant="primary"
						onClick={onSubmit}
						disabled={!valid || busy}
					>
						{busy ? "sending…" : "send"}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
