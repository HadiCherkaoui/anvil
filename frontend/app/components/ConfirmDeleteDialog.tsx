"use client";

import { useState, type ReactElement } from "react";

import { ApiError } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";

export interface ConfirmDeleteDialogProps {
	open: boolean;
	onClose: () => void;
	/** The string the user must type to enable confirm. */
	targetName: string;
	/** Optional override for the verb shown on the busy button. Default: "deleting…". */
	busyLabel?: string;
	/** Optional title; defaults to `delete ${targetName}?`. */
	title?: string;
	/** Optional explanatory paragraph above the input. */
	description?: ReactElement | string;
	/** Called when the user clicks confirm. The dialog closes on resolve. */
	onConfirm: () => Promise<void>;
}

/**
 * Generic "type the name to confirm" destructive dialog. Used for both
 * server delete (sub-project A) and recursive folder delete (sub-project
 * D). Caller owns the API call; this component owns the typed-name
 * pattern, the busy state, and the Modal lifecycle.
 */
export function ConfirmDeleteDialog({
	open,
	onClose,
	targetName,
	busyLabel = "deleting…",
	title,
	description,
	onConfirm,
}: ConfirmDeleteDialogProps): ReactElement {
	const [typed, setTyped] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const matches = typed === targetName;
	const noop = (): void => undefined;

	const handleClose = (): void => {
		// Reset on close so re-opening the dialog gives a clean slate
		// (matches sub-project A behaviour pre-refactor).
		setTyped("");
		setBusy(false);
		setError(null);
		onClose();
	};

	const handleConfirm = (): void => {
		if (!matches || busy) return;
		setError(null);
		setBusy(true);
		onConfirm()
			.then(() => {
				setTyped("");
				onClose();
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
		<Modal
			open={open}
			onClose={busy ? noop : handleClose}
			title={title ?? `delete ${targetName}?`}
		>
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				{description !== undefined && (
					<p className="text-text-body">{description}</p>
				)}
				<label className="flex flex-col gap-1.5">
					<span className="text-[11px] uppercase tracking-wider text-text-muted">
						type <span className="text-text-primary">{targetName}</span> to
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
					<Button onClick={handleClose} disabled={busy}>
						cancel
					</Button>
					<Button
						variant="danger"
						onClick={handleConfirm}
						disabled={!matches || busy}
					>
						{busy ? busyLabel : "delete"}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
