// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useState, type ReactElement } from "react";

import {
	ApiError,
	gamemodeSchema,
	runPlayerAction,
	type Gamemode,
	type PlayerAction,
} from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

// The variant determines which input (if any) the dialog renders.
export type PlayerActionVariant =
	| { kind: "kick"; player: string }
	| { kind: "ban"; player: string }
	| { kind: "ban-ip"; player: string }
	| { kind: "pardon"; player: string }
	| { kind: "pardon-ip"; ip: string }
	| { kind: "whitelist-remove"; player: string }
	| { kind: "gamemode"; player: string }
	| { kind: "tell"; player: string };

interface PlayerActionDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
	variant: PlayerActionVariant | null;
	onDone: () => void;
}

const REASON_MAX = 100;
const MESSAGE_MAX = 256;
const CONTROL = /[\x00-\x1f\x7f]/;

const TITLE: Record<PlayerActionVariant["kind"], string> = {
	kick: "kick player",
	ban: "ban player",
	"ban-ip": "ban player + ip",
	pardon: "pardon player",
	"pardon-ip": "pardon ip",
	"whitelist-remove": "remove from whitelist",
	gamemode: "change gamemode",
	tell: "send /tell",
};

const VERB_PRESENT: Record<PlayerActionVariant["kind"], string> = {
	kick: "kicking",
	ban: "banning",
	"ban-ip": "banning",
	pardon: "pardoning",
	"pardon-ip": "pardoning",
	"whitelist-remove": "removing",
	gamemode: "applying",
	tell: "sending",
};

const SUCCESS_TOAST: Record<
	PlayerActionVariant["kind"],
	(target: string) => string
> = {
	kick: (t) => `kicked ${t}`,
	ban: (t) => `banned ${t}`,
	"ban-ip": (t) => `banned ${t} (ip)`,
	pardon: (t) => `pardoned ${t}`,
	"pardon-ip": (t) => `pardoned ${t}`,
	"whitelist-remove": (t) => `removed ${t} from whitelist`,
	gamemode: (t) => `set ${t}'s gamemode`,
	tell: (t) => `sent message to ${t}`,
};

const DANGER_KINDS: ReadonlyArray<PlayerActionVariant["kind"]> = [
	"kick",
	"ban",
	"ban-ip",
	"pardon",
	"pardon-ip",
	"whitelist-remove",
];

export function PlayerActionDialog({
	open,
	onClose,
	serverId,
	variant,
	onDone,
}: PlayerActionDialogProps): ReactElement | null {
	const [reason, setReason] = useState("");
	const [message, setMessage] = useState("");
	const [mode, setMode] = useState<Gamemode>("survival");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const toast = useToast();

	if (variant === null) return null;

	const target = variant.kind === "pardon-ip" ? variant.ip : variant.player;

	const reasonValid = reason.length <= REASON_MAX && !CONTROL.test(reason);
	const messageByteLen = new TextEncoder().encode(message).byteLength;
	const messageValid =
		message.length > 0 &&
		messageByteLen <= MESSAGE_MAX &&
		!CONTROL.test(message);

	const valid =
		variant.kind === "kick" ||
		variant.kind === "ban" ||
		variant.kind === "ban-ip"
			? reasonValid
			: variant.kind === "tell"
				? messageValid
				: variant.kind === "gamemode"
					? gamemodeSchema.options.includes(mode)
					: true;

	const reset = (): void => {
		setReason("");
		setMessage("");
		setMode("survival");
		setError(null);
	};

	const handleClose = (): void => {
		reset();
		onClose();
	};

	const buildAction = (): PlayerAction | null => {
		switch (variant.kind) {
			case "kick":
				return {
					action: "kick",
					player: variant.player,
					reason: reason.length > 0 ? reason : undefined,
				};
			case "ban":
				return {
					action: "ban",
					player: variant.player,
					reason: reason.length > 0 ? reason : undefined,
				};
			case "ban-ip":
				return {
					action: "ban-ip",
					player: variant.player,
					reason: reason.length > 0 ? reason : undefined,
				};
			case "pardon":
				return { action: "pardon", player: variant.player };
			case "pardon-ip":
				return { action: "pardon-ip", ip: variant.ip };
			case "whitelist-remove":
				return { action: "whitelist-remove", player: variant.player };
			case "gamemode":
				return { action: "gamemode", player: variant.player, mode };
			case "tell":
				return { action: "tell", player: variant.player, message };
		}
	};

	const onSubmit = (): void => {
		const a = buildAction();
		if (a === null || !valid) return;
		setError(null);
		setBusy(true);
		void runPlayerAction(serverId, a)
			.then(() => {
				toast.push(SUCCESS_TOAST[variant.kind](target), "success");
				onDone();
				reset();
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

	const danger = DANGER_KINDS.includes(variant.kind);

	return (
		<Modal
			open={open}
			onClose={handleClose}
			title={TITLE[variant.kind]}
			maxWidth="md"
		>
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				<p className="text-text-body">
					{VERB_PRESENT[variant.kind]}{" "}
					<span className="text-text-primary">{target}</span>
				</p>

				{(variant.kind === "kick" ||
					variant.kind === "ban" ||
					variant.kind === "ban-ip") && (
					<label className="flex flex-col gap-1.5">
						<span className="text-[11px] uppercase tracking-wider text-text-muted">
							reason (optional)
						</span>
						<input
							type="text"
							value={reason}
							onChange={(e) => {
								setReason(e.target.value);
							}}
							maxLength={REASON_MAX}
							autoFocus
							className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						/>
						<span className="text-[11px] text-text-dim">
							{reason.length} / {REASON_MAX}
						</span>
					</label>
				)}

				{variant.kind === "gamemode" && (
					<label className="flex flex-col gap-1.5">
						<span className="text-[11px] uppercase tracking-wider text-text-muted">
							mode
						</span>
						<select
							value={mode}
							onChange={(e) => {
								setMode(e.target.value as Gamemode);
							}}
							autoFocus
							className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						>
							{gamemodeSchema.options.map((m) => (
								<option key={m} value={m}>
									{m}
								</option>
							))}
						</select>
					</label>
				)}

				{variant.kind === "tell" && (
					<label className="flex flex-col gap-1.5">
						<span className="text-[11px] uppercase tracking-wider text-text-muted">
							message
						</span>
						<textarea
							value={message}
							onChange={(e) => {
								// Strip newlines — the tell endpoint rejects control
								// chars and /tell is single-line.
								setMessage(e.target.value.replace(/[\n\r]/g, ""));
							}}
							rows={3}
							autoFocus
							className="w-full resize-none rounded-md border border-border bg-bg px-3 py-2 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						/>
						<span className="text-[11px] text-text-dim">
							{messageByteLen} / {MESSAGE_MAX}
						</span>
					</label>
				)}

				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={handleClose} disabled={busy}>
						cancel
					</Button>
					<Button
						variant={danger ? "danger" : "primary"}
						onClick={onSubmit}
						disabled={!valid || busy}
					>
						{busy ? "…" : variant.kind}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
