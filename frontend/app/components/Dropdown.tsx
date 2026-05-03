"use client";

import {
	useEffect,
	useRef,
	useState,
	type ReactElement,
	type ReactNode,
} from "react";

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

export function Dropdown({
	trigger,
	items,
	ariaLabel,
}: DropdownProps): ReactElement {
	const [open, setOpen] = useState(false);
	const wrapRef = useRef<HTMLDivElement>(null);

	useEffect(() => {
		if (!open) return undefined;
		const onClick = (event: MouseEvent): void => {
			if (wrapRef.current && !wrapRef.current.contains(event.target as Node)) {
				setOpen(false);
			}
		};
		const onKey = (event: KeyboardEvent): void => {
			if (event.key === "Escape") setOpen(false);
		};
		document.addEventListener("click", onClick);
		document.addEventListener("keydown", onKey);
		return () => {
			document.removeEventListener("click", onClick);
			document.removeEventListener("keydown", onKey);
		};
	}, [open]);

	return (
		<div ref={wrapRef} className="relative">
			<button
				type="button"
				aria-label={ariaLabel}
				aria-haspopup="menu"
				aria-expanded={open}
				onClick={() => {
					setOpen((v) => !v);
				}}
				className="inline-flex h-7 w-7 items-center justify-center rounded text-text-muted transition-colors hover:bg-elevated focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			>
				{trigger}
			</button>
			{open && (
				<div
					role="menu"
					className="absolute right-0 z-10 mt-1 min-w-40 rounded-md border border-border bg-surface py-1 shadow-lg"
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
								"block w-full px-3 py-1.5 text-left font-mono text-[12px] transition-colors hover:bg-elevated focus-visible:outline-none focus-visible:bg-elevated",
								it.danger === true ? "text-state-error" : "text-text-body",
							)}
						>
							{it.label}
						</button>
					))}
				</div>
			)}
		</div>
	);
}
