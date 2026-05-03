"use client";

import {
	createContext,
	useCallback,
	useContext,
	useState,
	type ReactElement,
	type ReactNode,
} from "react";

import { cn } from "../lib/cn";

type Tone = "info" | "success" | "error";

interface Toast {
	id: number;
	message: string;
	tone: Tone;
}

interface ToastCtx {
	push: (message: string, tone?: Tone) => void;
}

const Ctx = createContext<ToastCtx | null>(null);

const TONE_CLASS: Record<Tone, string> = {
	info: "border-border text-text-body",
	success: "border-state-running text-state-running",
	error: "border-state-error text-state-error",
};

const TOAST_TTL_MS = 4000;

export function ToastProvider({
	children,
}: {
	children: ReactNode;
}): ReactElement {
	const [toasts, setToasts] = useState<Toast[]>([]);
	const push = useCallback((message: string, tone: Tone = "info"): void => {
		const id = Date.now() + Math.random();
		setToasts((prev) => [...prev, { id, message, tone }]);
		window.setTimeout(() => {
			setToasts((prev) => prev.filter((t) => t.id !== id));
		}, TOAST_TTL_MS);
	}, []);
	return (
		<Ctx.Provider value={{ push }}>
			{children}
			<div className="pointer-events-none fixed bottom-4 right-4 z-50 flex flex-col gap-2">
				{toasts.map((t) => (
					<div
						key={t.id}
						role="status"
						className={cn(
							"pointer-events-auto rounded-md border bg-surface px-3 py-2 font-mono text-[12px] shadow-lg",
							TONE_CLASS[t.tone],
						)}
					>
						{t.message}
					</div>
				))}
			</div>
		</Ctx.Provider>
	);
}

export function useToast(): ToastCtx {
	const v = useContext(Ctx);
	if (!v) {
		throw new Error("useToast must be used inside ToastProvider");
	}
	return v;
}
