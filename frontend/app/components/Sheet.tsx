"use client";

import { useEffect, useRef, type ReactElement, type ReactNode } from "react";

import { cn } from "../lib/cn";

type Width = 480 | 640 | 720;

interface SheetProps {
	isOpen: boolean;
	onClose: () => void;
	title: string;
	width?: Width;
	children: ReactNode;
}

export function Sheet({
	isOpen,
	onClose,
	title,
	width = 480,
	children,
}: SheetProps): ReactElement {
	const panelRef = useRef<HTMLDivElement>(null);

	useEffect(() => {
		if (!isOpen) return undefined;
		const onKey = (event: KeyboardEvent): void => {
			if (event.key === "Escape") onClose();
		};
		document.addEventListener("keydown", onKey);
		return () => {
			document.removeEventListener("keydown", onKey);
		};
	}, [isOpen, onClose]);

	useEffect(() => {
		if (!isOpen) return undefined;
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
			// Re-query on every Tab — sheets populate async content (search
			// hits, version lists) that a one-shot snapshot would miss.
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
	}, [isOpen]);

	return (
		<div
			className={cn(
				"fixed inset-0 z-50 transition-opacity",
				isOpen
					? "pointer-events-auto opacity-100"
					: "pointer-events-none opacity-0",
			)}
			style={{ transitionDuration: "var(--motion-default)" }}
			aria-hidden={!isOpen}
		>
			<button
				type="button"
				aria-label="close sheet"
				className="absolute inset-0 bg-bg/70"
				onClick={onClose}
				tabIndex={-1}
			/>
			<div
				ref={panelRef}
				role="dialog"
				aria-modal="true"
				aria-label={title}
				className={cn(
					"absolute right-0 top-0 h-full border-l border-border bg-surface shadow-2xl",
					"transition-transform",
					isOpen ? "translate-x-0" : "translate-x-full",
				)}
				style={{
					width: `${String(width)}px`,
					transitionDuration: "var(--motion-slow)",
				}}
			>
				<header className="flex items-center justify-between border-b border-border-soft px-5 py-3">
					<h2 className="font-mono text-[13px] uppercase tracking-wider text-text-primary">
						{title}
					</h2>
					<button
						type="button"
						onClick={onClose}
						aria-label="close"
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
				</header>
				<div
					className="overflow-y-auto"
					style={{ height: "calc(100% - 49px)" }}
				>
					{children}
				</div>
			</div>
		</div>
	);
}
