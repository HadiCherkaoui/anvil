"use client";

import { useEffect, useRef, type ReactElement, type ReactNode } from "react";

import { cn } from "../lib/cn";

interface ModalProps {
	open: boolean;
	onClose: () => void;
	title: string;
	children: ReactNode;
	maxWidth?: "sm" | "md" | "lg";
}

const MAX_WIDTHS: Record<NonNullable<ModalProps["maxWidth"]>, string> = {
	sm: "max-w-sm",
	md: "max-w-md",
	lg: "max-w-lg",
};

export function Modal({
	open,
	onClose,
	title,
	children,
	maxWidth = "lg",
}: ModalProps): ReactElement | null {
	const panelRef = useRef<HTMLDivElement>(null);

	useEffect(() => {
		if (!open) return undefined;
		const onKey = (event: KeyboardEvent): void => {
			if (event.key === "Escape") onClose();
		};
		window.addEventListener("keydown", onKey);
		return () => {
			window.removeEventListener("keydown", onKey);
		};
	}, [open, onClose]);

	useEffect(() => {
		if (!open) return undefined;
		const previous = document.activeElement as HTMLElement | null;
		const panel = panelRef.current;
		if (!panel) return undefined;
		const queryFocusables = (): NodeListOf<HTMLElement> =>
			panel.querySelectorAll<HTMLElement>(
				'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
			);
		queryFocusables()[0]?.focus();
		const handler = (event: KeyboardEvent): void => {
			if (event.key !== "Tab") return;
			// Re-query on every Tab — children added after open (error rows,
			// async lists) would otherwise be unreachable in the trap.
			const focusables = queryFocusables();
			const first = focusables[0];
			const last = focusables[focusables.length - 1];
			if (!first || !last) return;
			if (event.shiftKey && document.activeElement === first) {
				last.focus();
				event.preventDefault();
			} else if (!event.shiftKey && document.activeElement === last) {
				first.focus();
				event.preventDefault();
			}
		};
		document.addEventListener("keydown", handler);
		return () => {
			document.removeEventListener("keydown", handler);
			previous?.focus();
		};
	}, [open]);

	if (!open) return null;

	return (
		<div
			className="fixed inset-0 z-50 flex items-center justify-center bg-bg/70 p-6"
			onClick={onClose}
			role="dialog"
			aria-modal="true"
			aria-labelledby="modal-title"
		>
			<div
				ref={panelRef}
				className={cn(
					"w-full rounded-md border border-border bg-surface p-6 shadow-2xl",
					MAX_WIDTHS[maxWidth],
				)}
				onClick={(event) => {
					event.stopPropagation();
				}}
			>
				<div className="mb-4 flex items-center justify-between">
					<h2
						id="modal-title"
						className="font-mono text-[13px] uppercase tracking-wider text-text-primary"
					>
						{title}
					</h2>
					<button
						type="button"
						onClick={onClose}
						aria-label="Close"
						className="rounded-sm text-text-muted transition-colors hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
					>
						<svg
							viewBox="0 0 24 24"
							width="20"
							height="20"
							fill="none"
							stroke="currentColor"
							strokeWidth={2}
						>
							<path d="M6 6l12 12M18 6L6 18" />
						</svg>
					</button>
				</div>
				{children}
			</div>
		</div>
	);
}
