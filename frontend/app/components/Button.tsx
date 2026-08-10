// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import type { ButtonHTMLAttributes, ReactElement, ReactNode } from "react";

import { cn } from "../lib/cn";

type Variant = "primary" | "secondary" | "danger" | "ghost";
type Size = "sm" | "md";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
	variant?: Variant;
	size?: Size;
	children: ReactNode;
}

// `cursor-pointer` is on the BASE because the browser UA stylesheet sets
// `<button>` cursor to `default`; without overriding it, hover gives the
// arrow cursor and the user thinks the element isn't clickable.
const BASE =
	"inline-flex items-center gap-2 rounded-md font-mono uppercase tracking-wide " +
	"cursor-pointer transition-colors " +
	"disabled:opacity-40 disabled:cursor-not-allowed " +
	"focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent";

const SIZES: Record<Size, string> = {
	sm: "px-2 py-1 text-[11px]",
	md: "px-3 py-1.5 text-xs",
};

const VARIANTS: Record<Variant, string> = {
	primary:
		"bg-accent-bg border border-accent-border text-accent hover:border-accent",
	secondary:
		"bg-surface border border-border text-text-body hover:border-border-strong",
	danger:
		"bg-surface border border-border text-state-error hover:border-state-error",
	ghost: "text-text-muted hover:text-text-body",
};

export function Button({
	variant = "secondary",
	size = "md",
	children,
	className,
	type = "button",
	disabled,
	...rest
}: ButtonProps): ReactElement {
	const inner =
		variant === "primary" ? (
			<>
				<span className="text-accent-bracket">[</span>
				{children}
				<span className="text-accent-bracket">]</span>
			</>
		) : (
			children
		);
	return (
		<button
			type={type}
			disabled={disabled}
			className={cn(BASE, SIZES[size], VARIANTS[variant], className)}
			{...rest}
		>
			{inner}
		</button>
	);
}
