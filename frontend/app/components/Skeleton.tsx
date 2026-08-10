// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactElement } from "react";

import { cn } from "../lib/cn";

type Variant = "row" | "block" | "text";

interface SkeletonProps {
	variant: Variant;
	className?: string;
}

const SHAPES: Record<Variant, string> = {
	row: "h-10 w-full",
	block: "h-32 w-full rounded-md",
	text: "h-3 w-24 rounded-sm",
};

export function Skeleton({ variant, className }: SkeletonProps): ReactElement {
	return (
		<div
			className={cn("animate-pulse bg-elevated", SHAPES[variant], className)}
			aria-hidden="true"
		/>
	);
}
