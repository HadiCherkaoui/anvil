// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import {
	useCallback,
	useState,
	type FormEvent,
	type ReactElement,
} from "react";

import { ApiError, sendRconCommand } from "../lib/api";

import { Button } from "./Button";

interface RconCommandProps {
	readonly serverId: string;
	readonly disabled?: boolean;
}

export function RconCommand({
	serverId,
	disabled = false,
}: RconCommandProps): ReactElement {
	const [cmd, setCmd] = useState("");
	const [output, setOutput] = useState<string | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);

	const handleSubmit = useCallback(
		(ev: FormEvent<HTMLFormElement>): void => {
			ev.preventDefault();
			if (cmd.trim().length === 0) return;
			setBusy(true);
			setError(null);
			void sendRconCommand(serverId, cmd)
				.then(({ output: text }) => {
					setOutput(text);
				})
				.catch((err: unknown) => {
					setOutput(null);
					if (err instanceof ApiError) {
						setError(err.message);
					} else if (err instanceof Error) {
						setError(err.message);
					} else {
						setError("unknown error");
					}
				})
				.finally(() => {
					setBusy(false);
				});
		},
		[cmd, serverId],
	);

	const inputDisabled = disabled || busy;

	return (
		<section className="flex flex-col gap-3 rounded-md border border-border bg-surface p-4">
			<h2 className="font-mono text-[11px] uppercase tracking-wider text-text-muted">
				send command
			</h2>
			<form onSubmit={handleSubmit} className="flex gap-2">
				<input
					type="text"
					value={cmd}
					onChange={(ev) => {
						setCmd(ev.target.value);
					}}
					placeholder={disabled ? "server is not running" : "say hi"}
					disabled={inputDisabled}
					className="flex-1 rounded-md border border-border bg-bg px-3 py-1.5 font-mono text-[13px] text-text-body placeholder:text-text-faint focus:border-border-strong focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent disabled:opacity-50"
					autoComplete="off"
					spellCheck={false}
				/>
				<Button
					variant="primary"
					type="submit"
					disabled={inputDisabled || cmd.trim().length === 0}
				>
					send
				</Button>
			</form>
			{error !== null && (
				<p className="font-mono text-[12px] text-state-error">{error}</p>
			)}
			{output !== null && error === null && (
				<pre className="max-h-40 overflow-auto rounded-sm bg-bg p-3 font-mono text-[12px] leading-relaxed text-text-body">
					{output.length === 0 ? "(empty response)" : output}
				</pre>
			)}
		</section>
	);
}
