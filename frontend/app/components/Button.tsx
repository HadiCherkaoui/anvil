"use client";

import type { ButtonHTMLAttributes, ReactElement, ReactNode } from "react";

type Variant = "primary" | "secondary" | "danger";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
	variant?: Variant;
	children: ReactNode;
}

const VARIANT_CLASSES: Record<Variant, string> = {
	primary:
		"bg-green-500/20 text-green-300 hover:bg-green-500/30 disabled:bg-green-500/10 disabled:text-green-300/40",
	secondary:
		"bg-slate-700/40 text-slate-200 hover:bg-slate-700/60 disabled:bg-slate-700/20 disabled:text-slate-200/40",
	danger:
		"bg-red-500/20 text-red-300 hover:bg-red-500/30 disabled:bg-red-500/10 disabled:text-red-300/40",
};

export function Button({
	variant = "primary",
	children,
	className,
	type = "button",
	disabled,
	...rest
}: ButtonProps): ReactElement {
	const variantClass = VARIANT_CLASSES[variant];
	const baseClass =
		"inline-flex items-center gap-2 rounded-md px-3 py-1.5 text-sm font-medium transition-colors disabled:cursor-not-allowed";
	return (
		<button
			type={type}
			disabled={disabled}
			className={`${baseClass} ${variantClass} ${className ?? ""}`.trim()}
			{...rest}
		>
			{children}
		</button>
	);
}
