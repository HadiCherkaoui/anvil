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
			sendRconCommand(serverId, cmd)
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

	return (
		<section className="flex flex-col gap-3 rounded-lg border border-slate-800 p-4">
			<h2 className="text-xs uppercase tracking-wide text-slate-400">
				Send command
			</h2>
			<form onSubmit={handleSubmit} className="flex gap-2">
				<input
					type="text"
					value={cmd}
					onChange={(ev) => {
						setCmd(ev.target.value);
					}}
					placeholder="say hi"
					disabled={disabled || busy}
					className="flex-1 rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 font-mono text-sm text-slate-100 placeholder:text-slate-500 focus:border-slate-500 focus:outline-none disabled:opacity-50"
					autoComplete="off"
					spellCheck={false}
				/>
				<Button
					variant="primary"
					type="submit"
					disabled={disabled || busy || cmd.trim().length === 0}
				>
					send
				</Button>
			</form>
			{error !== null && <p className="text-sm text-red-400">{error}</p>}
			{output !== null && error === null && (
				<pre className="max-h-40 overflow-auto rounded-md bg-slate-950 p-3 font-mono text-xs leading-relaxed text-slate-300">
					{output.length === 0 ? "(empty response)" : output}
				</pre>
			)}
		</section>
	);
}
