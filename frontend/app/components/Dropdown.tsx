// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import {
	useEffect,
	useRef,
	useState,
	type ReactElement,
	type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { cn } from "../lib/cn";

export interface DropdownItem {
	id: string;
	label: string;
	onSelect: () => void;
	danger?: boolean;
}

interface DropdownProps {
	trigger: ReactNode;
	items: ReadonlyArray<DropdownItem>;
	ariaLabel: string;
}

interface MenuPosition {
	top: number;
	right: number;
}

// Portal-based menu: trigger lives in the table row but the dropdown
// renders into <body> with fixed positioning, so an `overflow-hidden`
// parent (rounded table wrapper) cannot clip it.
export function Dropdown({
	trigger,
	items,
	ariaLabel,
}: DropdownProps): ReactElement {
	const [open, setOpen] = useState(false);
	const [pos, setPos] = useState<MenuPosition | null>(null);
	const triggerRef = useRef<HTMLButtonElement>(null);
	const menuRef = useRef<HTMLDivElement>(null);

	const updatePos = (): void => {
		const el = triggerRef.current;
		if (!el) return;
		const rect = el.getBoundingClientRect();
		setPos({
			top: rect.bottom + 4,
			right: window.innerWidth - rect.right,
		});
	};

	useEffect(() => {
		if (!open) return undefined;
		const onClick = (event: MouseEvent): void => {
			const target = event.target as Node;
			if (
				!triggerRef.current?.contains(target) &&
				!menuRef.current?.contains(target)
			) {
				setOpen(false);
			}
		};
		const onKey = (event: KeyboardEvent): void => {
			if (event.key === "Escape") setOpen(false);
		};
		const onReposition = (): void => {
			updatePos();
		};
		document.addEventListener("click", onClick);
		document.addEventListener("keydown", onKey);
		window.addEventListener("scroll", onReposition, true);
		window.addEventListener("resize", onReposition);
		return () => {
			document.removeEventListener("click", onClick);
			document.removeEventListener("keydown", onKey);
			window.removeEventListener("scroll", onReposition, true);
			window.removeEventListener("resize", onReposition);
		};
	}, [open]);

	const handleToggle = (): void => {
		if (!open) updatePos();
		setOpen((v) => !v);
	};

	const menu =
		open && pos !== null ? (
			<div
				ref={menuRef}
				role="menu"
				style={{
					position: "fixed",
					top: `${pos.top.toString()}px`,
					right: `${pos.right.toString()}px`,
				}}
				className="z-50 min-w-40 rounded-md border border-border bg-surface py-1 shadow-lg"
			>
				{items.map((it) => (
					<button
						key={it.id}
						role="menuitem"
						type="button"
						onClick={() => {
							setOpen(false);
							it.onSelect();
						}}
						className={cn(
							"block w-full px-3 py-1.5 text-left font-mono text-[12px] transition-colors hover:bg-elevated focus-visible:bg-elevated focus-visible:outline-none",
							it.danger === true ? "text-state-error" : "text-text-body",
						)}
					>
						{it.label}
					</button>
				))}
			</div>
		) : null;

	return (
		<>
			<button
				ref={triggerRef}
				type="button"
				aria-label={ariaLabel}
				aria-haspopup="menu"
				aria-expanded={open}
				onClick={handleToggle}
				className="inline-flex h-7 w-7 items-center justify-center rounded text-text-muted transition-colors hover:bg-elevated focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			>
				{trigger}
			</button>
			{menu !== null && typeof document !== "undefined"
				? createPortal(menu, document.body)
				: null}
		</>
	);
}
