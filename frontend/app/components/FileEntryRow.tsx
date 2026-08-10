// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { type ReactElement } from "react";

import { type FileEntry, type FileEntryType } from "../lib/api";

import { FileActionMenu } from "./FileActionMenu";

export interface FileEntryRowProps {
	entry: FileEntry;
	onNavigate: (toPath: string) => void;
	onDownload: () => void;
	onRename: () => void;
	onDelete: () => void;
	/** Current directory path, used to construct the navigated-to path. */
	parentPath: string;
}

function humanSize(bytes: number, type: FileEntryType): string {
	if (type === "d") return "─";
	if (bytes < 1024) return `${bytes.toString()} B`;
	if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
	if (bytes < 1024 * 1024 * 1024)
		return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
	return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GiB`;
}

function relativeTime(unixSeconds: number): string {
	const diff = Math.round(Date.now() / 1000 - unixSeconds);
	if (diff < 60) return `${diff.toString()}s ago`;
	if (diff < 3600) return `${Math.floor(diff / 60).toString()}m ago`;
	if (diff < 86400) return `${Math.floor(diff / 3600).toString()}h ago`;
	return `${Math.floor(diff / 86400).toString()}d ago`;
}

function glyph(type: FileEntryType): string {
	switch (type) {
		case "d":
			return "/";
		case "l":
			return "→";
		case "f":
			return " ";
		case "o":
		default:
			return " ";
	}
}

export function FileEntryRow({
	entry,
	onNavigate,
	onDownload,
	onRename,
	onDelete,
	parentPath,
}: FileEntryRowProps): ReactElement {
	const handleNameClick = (): void => {
		if (entry.type === "d") {
			const next =
				parentPath === "/" ? `/${entry.name}` : `${parentPath}/${entry.name}`;
			onNavigate(next);
		} else if (entry.type === "f") {
			onDownload();
		}
	};

	return (
		<div className="grid grid-cols-[auto_1fr_auto_auto_auto] items-center gap-3 px-3 py-1 hover:bg-elevated">
			<span className="w-3 text-center font-mono text-[12px] text-text-muted">
				{glyph(entry.type)}
			</span>
			<button
				type="button"
				onClick={handleNameClick}
				className="text-left font-mono text-[12px] text-text-body hover:text-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			>
				{entry.name}
			</button>
			<span className="font-mono text-[12px] text-text-muted">
				{humanSize(entry.size, entry.type)}
			</span>
			<span className="font-mono text-[12px] text-text-muted">
				{relativeTime(entry.mtime)}
			</span>
			<FileActionMenu
				entryType={entry.type}
				onDownload={onDownload}
				onRename={onRename}
				onDelete={onDelete}
			/>
		</div>
	);
}
