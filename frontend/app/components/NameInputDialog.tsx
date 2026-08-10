// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useState, type ReactElement } from "react";

import { ApiError } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";

export interface NameInputDialogProps {
	open: boolean;
	onClose: () => void;
	mode: "create" | "rename";
	/** Empty string for create mode; the existing name for rename. */
	initialValue: string;
	/** Called with the typed name when the user submits. */
	onSubmit: (name: string) => Promise<void>;
}

const PRINTABLE_ASCII = /^[\x20-\x7E]+$/;

function validateSegment(name: string): string | null {
	if (name.length === 0) return "name cannot be empty";
	if (name === "." || name === "..") return "'.' and '..' are reserved";
	if (name.startsWith("-")) return "name may not start with '-'";
	if (name.length > 255) return "name too long (max 255 bytes)";
	if (name.includes("/")) return "name may not contain '/'";
	if (!PRINTABLE_ASCII.test(name)) return "only printable ASCII allowed";
	if (name.includes("'") || name.includes("\\")) {
		return "single-quotes and backslashes are not allowed";
	}
	return null;
}

/**
 * Mkdir + rename dialog. Caller MUST pass a stable `key` prop derived
 * from `initialValue` (e.g. `key={renameTarget?.name ?? "create"}`) so
 * React re-mounts the component when the target changes — this is how
 * we reset state without triggering the React 19 setState-in-effect
 * lint.
 */
export function NameInputDialog({
	open,
	onClose,
	mode,
	initialValue,
	onSubmit,
}: NameInputDialogProps): ReactElement {
	const [value, setValue] = useState(initialValue);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const noop = (): void => undefined;

	const validation = validateSegment(value);
	const canSubmit = validation === null && !busy;

	const handleSubmit = (): void => {
		if (!canSubmit) return;
		setBusy(true);
		setError(null);
		onSubmit(value)
			.then(() => {
				setBusy(false);
				onClose();
			})
			.catch((err: unknown) => {
				const message =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "operation failed";
				setError(message);
				setBusy(false);
			});
	};

	const handleClose = (): void => {
		setBusy(false);
		setError(null);
		onClose();
	};

	const title = mode === "create" ? "new folder" : "rename";
	const submitLabel = mode === "create" ? "create" : "rename";

	return (
		<Modal open={open} onClose={busy ? noop : handleClose} title={title}>
			<div className="flex flex-col gap-3 font-mono text-[13px]">
				<input
					type="text"
					value={value}
					onChange={(e) => {
						setValue(e.target.value);
						setError(null);
					}}
					autoFocus
					disabled={busy}
					className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
					placeholder={mode === "create" ? "folder-name" : ""}
				/>
				{validation !== null && value.length > 0 && (
					<p className="text-state-warning">{validation}</p>
				)}
				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={handleClose} disabled={busy}>
						cancel
					</Button>
					<Button
						variant="primary"
						onClick={handleSubmit}
						disabled={!canSubmit}
					>
						{busy ? `${submitLabel}…` : submitLabel}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
