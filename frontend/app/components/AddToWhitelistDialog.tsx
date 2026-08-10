// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useState, type ReactElement } from "react";

import { ApiError, runPlayerAction } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

interface AddToWhitelistDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
	onDone: () => void;
}

const NAME_REGEX = /^[A-Za-z0-9_]{3,16}$/;

export function AddToWhitelistDialog({
	open,
	onClose,
	serverId,
	onDone,
}: AddToWhitelistDialogProps): ReactElement {
	const [name, setName] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const toast = useToast();

	const valid = NAME_REGEX.test(name);

	const handleClose = (): void => {
		setName("");
		setError(null);
		onClose();
	};

	const onSubmit = (): void => {
		if (!valid) return;
		setError(null);
		setBusy(true);
		void runPlayerAction(serverId, { action: "whitelist-add", player: name })
			.then(() => {
				toast.push(`whitelisted ${name}`, "success");
				onDone();
				setName("");
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
			title="add to whitelist"
			maxWidth="sm"
		>
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				<label className="flex flex-col gap-1.5">
					<span className="text-[11px] uppercase tracking-wider text-text-muted">
						mojang username
					</span>
					<input
						type="text"
						value={name}
						onChange={(e) => {
							setName(e.target.value);
						}}
						autoFocus
						placeholder="alice"
						className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
					/>
					<span className="text-[11px] text-text-dim">
						3–16 chars, letters / digits / underscore
					</span>
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
						{busy ? "adding…" : "add"}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
