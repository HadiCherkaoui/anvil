"use client";

import { useEffect, type ReactElement, type ReactNode } from "react";

interface ModalProps {
	open: boolean;
	onClose: () => void;
	title: string;
	children: ReactNode;
}

export function Modal({
	open,
	onClose,
	title,
	children,
}: ModalProps): ReactElement | null {
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

	if (!open) return null;

	return (
		<div
			className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/70 p-6"
			onClick={onClose}
			role="dialog"
			aria-modal="true"
			aria-labelledby="modal-title"
		>
			<div
				className="w-full max-w-lg rounded-lg border border-slate-700 bg-slate-900 p-6 shadow-2xl"
				onClick={(event) => {
					event.stopPropagation();
				}}
			>
				<div className="mb-4 flex items-center justify-between">
					<h2
						id="modal-title"
						className="text-base font-semibold tracking-tight"
					>
						{title}
					</h2>
					<button
						type="button"
						onClick={onClose}
						aria-label="Close"
						className="text-slate-400 hover:text-slate-100"
					>
						✕
					</button>
				</div>
				{children}
			</div>
		</div>
	);
}
