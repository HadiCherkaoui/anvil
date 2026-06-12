"use client";

import { type ReactElement } from "react";

import { ApiError, runPlayerAction } from "../lib/api";

import { Dropdown, type DropdownItem } from "./Dropdown";
import type { PlayerActionVariant } from "./PlayerActionDialog";
import { useToast } from "./Toast";

export type PlayerActionSource = "online" | "whitelist" | "banlist";

interface PlayerActionMenuProps {
	source: PlayerActionSource;
	serverId: string;
	// Player username (for online + whitelist + banlist-player rows).
	name?: string;
	// IP for banlist-ip rows; mutually exclusive with `name`.
	ip?: string;
	// Open the shared PlayerActionDialog with the given variant.
	openDialog: (variant: PlayerActionVariant) => void;
	// Trigger an out-of-band poll after a fire-and-toast action.
	onDone: () => void;
}

const CHEVRON = (
	<svg
		viewBox="0 0 24 24"
		width="14"
		height="14"
		fill="none"
		stroke="currentColor"
		strokeWidth={2}
	>
		<circle cx="5" cy="12" r="1.5" />
		<circle cx="12" cy="12" r="1.5" />
		<circle cx="19" cy="12" r="1.5" />
	</svg>
);

export function PlayerActionMenu({
	source,
	serverId,
	name,
	ip,
	openDialog,
	onDone,
}: PlayerActionMenuProps): ReactElement {
	const toast = useToast();

	const fireAndToast = (
		label: string,
		message: string,
		action: () => Promise<void>,
	): void => {
		void action()
			.then(() => {
				toast.push(message, "success");
				onDone();
			})
			.catch((err: unknown) => {
				const detail =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`${label} failed: ${detail}`, "error");
			});
	};

	const items: DropdownItem[] = (() => {
		if (source === "online" && name !== undefined) {
			return [
				{
					id: "kick",
					label: "kick…",
					onSelect: () => {
						openDialog({ kind: "kick", player: name });
					},
				},
				{
					id: "op",
					label: "op",
					onSelect: () => {
						fireAndToast("op", `opped ${name}`, () =>
							runPlayerAction(serverId, { action: "op", player: name }),
						);
					},
				},
				{
					id: "deop",
					label: "deop",
					onSelect: () => {
						fireAndToast("deop", `deopped ${name}`, () =>
							runPlayerAction(serverId, { action: "deop", player: name }),
						);
					},
				},
				{
					id: "gamemode",
					label: "gamemode…",
					onSelect: () => {
						openDialog({ kind: "gamemode", player: name });
					},
				},
				{
					id: "tell",
					label: "/tell…",
					onSelect: () => {
						openDialog({ kind: "tell", player: name });
					},
				},
				{
					id: "whitelist-add",
					label: "add to whitelist",
					onSelect: () => {
						fireAndToast("whitelist add", `whitelisted ${name}`, () =>
							runPlayerAction(serverId, {
								action: "whitelist-add",
								player: name,
							}),
						);
					},
				},
				{
					id: "ban",
					label: "ban…",
					danger: true,
					onSelect: () => {
						openDialog({ kind: "ban", player: name });
					},
				},
				{
					id: "ban-ip",
					label: "ban-ip…",
					danger: true,
					onSelect: () => {
						openDialog({ kind: "ban-ip", player: name });
					},
				},
			];
		}
		if (source === "whitelist" && name !== undefined) {
			return [
				{
					id: "remove",
					label: "remove from whitelist…",
					danger: true,
					onSelect: () => {
						openDialog({ kind: "whitelist-remove", player: name });
					},
				},
				{
					id: "op",
					label: "op",
					onSelect: () => {
						fireAndToast("op", `opped ${name}`, () =>
							runPlayerAction(serverId, { action: "op", player: name }),
						);
					},
				},
				{
					id: "deop",
					label: "deop",
					onSelect: () => {
						fireAndToast("deop", `deopped ${name}`, () =>
							runPlayerAction(serverId, { action: "deop", player: name }),
						);
					},
				},
				{
					id: "ban",
					label: "ban…",
					danger: true,
					onSelect: () => {
						openDialog({ kind: "ban", player: name });
					},
				},
			];
		}
		if (source === "banlist") {
			if (name !== undefined) {
				return [
					{
						id: "pardon",
						label: "pardon…",
						onSelect: () => {
							openDialog({ kind: "pardon", player: name });
						},
					},
				];
			}
			if (ip !== undefined) {
				return [
					{
						id: "pardon-ip",
						label: "pardon ip…",
						onSelect: () => {
							openDialog({ kind: "pardon-ip", ip });
						},
					},
				];
			}
		}
		return [];
	})();

	return (
		<Dropdown trigger={CHEVRON} items={items} ariaLabel="player actions" />
	);
}
