// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useId, type ChangeEvent, type ReactElement } from "react";

import { cn } from "../lib/cn";

interface RangeSliderProps {
	label: string;
	value: number;
	onChange: (value: number) => void;
	min: number;
	max: number;
	step?: number;
	unit?: string;
	ticks?: ReadonlyArray<number>;
	className?: string;
}

export function RangeSlider({
	label,
	value,
	onChange,
	min,
	max,
	step = 1,
	unit,
	ticks,
	className,
}: RangeSliderProps): ReactElement {
	const inputId = useId();
	const handle = (event: ChangeEvent<HTMLInputElement>): void => {
		onChange(Number(event.target.value));
	};
	return (
		<div className={cn("flex flex-col gap-2", className)}>
			<div className="flex items-baseline justify-between">
				<label
					htmlFor={inputId}
					className="font-mono text-[11px] uppercase tracking-wider text-text-muted"
				>
					{label}
				</label>
				<span className="font-mono text-[12px] text-text-body">
					{value}
					{unit !== undefined && (
						<span className="ml-1 text-text-muted">{unit}</span>
					)}
				</span>
			</div>
			<input
				id={inputId}
				type="range"
				min={min}
				max={max}
				step={step}
				value={value}
				onChange={handle}
				className="h-1 w-full appearance-none rounded-full bg-border accent-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			/>
			{ticks !== undefined && ticks.length > 0 && (
				<div className="flex justify-between font-mono text-[10px] text-text-faint">
					{ticks.map((t) => (
						<span key={t}>{t}</span>
					))}
				</div>
			)}
		</div>
	);
}
