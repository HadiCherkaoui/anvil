import type { ReactElement } from "react";

import { cn } from "../lib/cn";

interface SegmentedControlProps<T extends string> {
	value: T;
	options: ReadonlyArray<{ value: T; label: string }>;
	onChange: (value: T) => void;
	ariaLabel: string;
	className?: string;
}

export function SegmentedControl<T extends string>({
	value,
	options,
	onChange,
	ariaLabel,
	className,
}: SegmentedControlProps<T>): ReactElement {
	return (
		<div
			role="radiogroup"
			aria-label={ariaLabel}
			className={cn(
				"inline-flex rounded-md border border-border bg-surface p-0.5",
				className,
			)}
		>
			{options.map((o) => {
				const active = o.value === value;
				return (
					<button
						key={o.value}
						type="button"
						role="radio"
						aria-checked={active}
						onClick={() => {
							onChange(o.value);
						}}
						className={cn(
							"rounded-sm px-2.5 py-1 font-mono text-[11px] uppercase tracking-wider transition-colors",
							"focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent",
							active
								? "bg-elevated text-text-primary"
								: "text-text-muted hover:text-text-body",
						)}
					>
						{o.label}
					</button>
				);
			})}
		</div>
	);
}
