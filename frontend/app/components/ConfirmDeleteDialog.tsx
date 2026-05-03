"use client";

import { useState, type ReactElement } from "react";

import { ApiError, deleteServer } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";

interface ConfirmDeleteDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
	serverName: string;
	onDeleted: () => void;
}

export function ConfirmDeleteDialog({
	open,
	onClose,
	serverId,
	serverName,
	onDeleted,
}: ConfirmDeleteDialogProps): ReactElement {
	const [typed, setTyped] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const matches = typed === serverName;

	const onConfirm = (): void => {
		setError(null);
		setBusy(true);
		deleteServer(serverId)
			.then(() => {
				onDeleted();
				setTyped("");
			})
			.catch((err: unknown) => {
				if (err instanceof ApiError) {
					setError(`${err.code}: ${err.message}`);
				} else {
					setError(err instanceof Error ? err.message : "unknown error");
				}
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<Modal open={open} onClose={onClose} title={`delete ${serverName}?`}>
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				<p className="text-text-body">
					this permanently removes the StatefulSet, PVC, Service, and RCON
					Secret. the server&apos;s world data is lost.
				</p>
				<label className="flex flex-col gap-1.5">
					<span className="text-[11px] uppercase tracking-wider text-text-muted">
						type <span className="text-text-primary">{serverName}</span> to
						confirm
					</span>
					<input
						type="text"
						value={typed}
						onChange={(e) => {
							setTyped(e.target.value);
						}}
						autoFocus
						className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-state-error"
					/>
				</label>
				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={onClose} disabled={busy}>
						cancel
					</Button>
					<Button
						variant="danger"
						onClick={onConfirm}
						disabled={!matches || busy}
					>
						{busy ? "deleting…" : "delete"}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
