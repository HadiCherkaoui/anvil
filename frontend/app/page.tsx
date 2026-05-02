"use client";

import { useCallback, useEffect, useState, type ReactElement } from "react";

import { Button } from "./components/Button";
import { NewServerModal } from "./components/NewServerModal";
import { ServerTable } from "./components/ServerTable";
import { ApiError, fetchServers, type ServerSummary } from "./lib/api";

const POLL_INTERVAL_MS = 5_000;

type LoadState =
	| { kind: "loading" }
	| { kind: "ready"; servers: readonly ServerSummary[] }
	| { kind: "error"; message: string };

export default function HomePage(): ReactElement {
	const [state, setState] = useState<LoadState>({ kind: "loading" });
	const [modalOpen, setModalOpen] = useState(false);

	const reload = useCallback(async (signal: AbortSignal): Promise<void> => {
		try {
			const servers = await fetchServers(signal);
			setState({ kind: "ready", servers });
		} catch (err: unknown) {
			if (err instanceof DOMException && err.name === "AbortError") return;
			const message =
				err instanceof ApiError
					? `${err.code}: ${err.message}`
					: err instanceof Error
						? err.message
						: "unknown error";
			setState({ kind: "error", message });
		}
	}, []);

	useEffect(() => {
		const ctrl = new AbortController();
		// Polling pattern: reload is async (setState is gated behind an
		// await fetch) so this is not a synchronous setState — the lint
		// rule overreaches here.
		// eslint-disable-next-line react-hooks/set-state-in-effect
		void reload(ctrl.signal);
		const handle = setInterval(() => {
			void reload(ctrl.signal);
		}, POLL_INTERVAL_MS);
		return () => {
			clearInterval(handle);
			ctrl.abort();
		};
	}, [reload]);

	const onActionDone = useCallback((): void => {
		const ctrl = new AbortController();
		void reload(ctrl.signal);
	}, [reload]);

	const onCreated = useCallback((): void => {
		setModalOpen(false);
		onActionDone();
	}, [onActionDone]);

	return (
		<main className="min-h-screen px-6 py-12">
			<header className="mx-auto mb-12 flex max-w-5xl items-baseline justify-between">
				<h1 className="text-2xl font-semibold tracking-tight">anvil</h1>
				<Button
					variant="primary"
					onClick={() => {
						setModalOpen(true);
					}}
				>
					+ new server
				</Button>
			</header>
			<section className="mx-auto max-w-5xl">
				{state.kind === "loading" && (
					<p className="text-sm text-slate-400">loading servers…</p>
				)}
				{state.kind === "error" && (
					<p className="text-sm text-red-400">
						failed to load servers: {state.message}
					</p>
				)}
				{state.kind === "ready" && (
					<ServerTable servers={state.servers} onActionDone={onActionDone} />
				)}
			</section>
			{modalOpen && (
				<NewServerModal
					open={modalOpen}
					onClose={() => {
						setModalOpen(false);
					}}
					onCreated={onCreated}
				/>
			)}
		</main>
	);
}
