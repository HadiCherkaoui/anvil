"use client";

import type { ButtonHTMLAttributes, ReactElement, ReactNode } from "react";

import { cn } from "../lib/cn";

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
	"aria-label": string;
	children: ReactNode;
}

export function IconButton({
	className,
	type = "button",
	children,
	...rest
}: IconButtonProps): ReactElement {
	return (
		<button
			type={type}
			className={cn(
				"inline-flex h-8 w-8 items-center justify-center rounded-md text-text-muted transition-colors",
				"hover:bg-elevated hover:text-text-body",
				"focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent",
				"disabled:opacity-40 disabled:cursor-not-allowed",
				className,
			)}
			{...rest}
		>
			{children}
		</button>
	);
}
