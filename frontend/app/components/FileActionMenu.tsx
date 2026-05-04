"use client";

import { type ReactElement } from "react";

import { type FileEntryType } from "../lib/api";

import { Dropdown, type DropdownItem } from "./Dropdown";

export interface FileActionMenuProps {
	entryType: FileEntryType;
	onDownload: () => void;
	onRename: () => void;
	onDelete: () => void;
}

/**
 * Per-entry-type action menu. File → download / rename / delete;
 * directory → rename / delete (recursive); symlink → rename / delete.
 * "Other" entries (sockets / fifos) get the same treatment as files
 * since the worst case is a useless download.
 */
export function FileActionMenu({
	entryType,
	onDownload,
	onRename,
	onDelete,
}: FileActionMenuProps): ReactElement {
	const items: DropdownItem[] = [];
	if (entryType === "f" || entryType === "o") {
		items.push({ id: "download", label: "download", onSelect: onDownload });
	}
	items.push({ id: "rename", label: "rename", onSelect: onRename });
	items.push({
		id: "delete",
		label: entryType === "d" ? "delete (recursive)" : "delete",
		onSelect: onDelete,
		danger: true,
	});
	return (
		<Dropdown
			ariaLabel="file actions"
			trigger={<span aria-hidden>⋯</span>}
			items={items}
		/>
	);
}
