// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useRouter } from "next/navigation";
import {
	useCallback,
	useEffect,
	useRef,
	useState,
	type ReactElement,
} from "react";

import { Button } from "./components/Button";
import { Card } from "./components/Card";
import { ServerList } from "./components/ServerList";
import { Skeleton } from "./components/Skeleton";
import { ApiError, fetchServers, type ServerSummary } from "./lib/api";

const POLL_INTERVAL_MS = 5_000;

type LoadState =
	| { kind: "loading" }
	| { kind: "ready"; servers: readonly ServerSummary[] }
	| { kind: "error"; message: string };

export default function HomePage(): ReactElement {
	const router = useRouter();
	const [state, setState] = useState<LoadState>({ kind: "loading" });
	const triggerReload = useRef<() => void>(() => undefined);

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
		let cancelled = false;
		let timer: number | undefined;
		// Each tick gets its own AbortController so a slow reply from the
		// previous tick can't leak into the next one's setState.
		let currentCtrl: AbortController | null = null;

		const fire = (): void => {
			if (cancelled) return;
			currentCtrl?.abort();
			const ctrl = new AbortController();
			currentCtrl = ctrl;
			void reload(ctrl.signal);
		};

		triggerReload.current = fire;

		const tick = (): void => {
			if (document.visibilityState === "visible") {
				fire();
			}
			timer = window.setTimeout(tick, POLL_INTERVAL_MS);
		};
		tick();

		const onVisibility = (): void => {
			if (document.visibilityState === "visible") {
				fire();
			}
		};
		document.addEventListener("visibilitychange", onVisibility);

		return () => {
			cancelled = true;
			if (timer !== undefined) window.clearTimeout(timer);
			document.removeEventListener("visibilitychange", onVisibility);
			currentCtrl?.abort();
		};
	}, [reload]);

	const onActionDone = useCallback((): void => {
		triggerReload.current();
	}, []);

	const summary = state.kind === "ready" ? buildSummary(state.servers) : null;

	return (
		<div className="px-6 py-8">
			<section className="mx-auto max-w-6xl">
				<header className="mb-6 flex items-baseline justify-between">
					<p className="font-mono text-[12px] text-text-muted">
						{summary ?? <Skeleton variant="text" className="h-3 w-64" />}
					</p>
					<Button
						variant="primary"
						onClick={() => {
							router.push("/servers/new");
						}}
					>
						+ new
					</Button>
				</header>

				{state.kind === "loading" && (
					<div className="flex flex-col gap-1 rounded-md border border-border bg-surface p-1">
						<Skeleton variant="row" />
						<Skeleton variant="row" />
						<Skeleton variant="row" />
					</div>
				)}
				{state.kind === "error" && (
					<Card>
						<p className="font-mono text-[12px] text-state-error">
							failed to load servers · {state.message}
						</p>
					</Card>
				)}
				{state.kind === "ready" && (
					<ServerList servers={state.servers} onActionDone={onActionDone} />
				)}
			</section>
		</div>
	);
}

function buildSummary(servers: readonly ServerSummary[]): string {
	const total = servers.length;
	const running = servers.filter((s) => s.status === "running").length;
	const stopped = servers.filter((s) => s.status === "stopped").length;
	const updates = servers.filter((s) => s.update_available).length;
	const parts = [
		`${total.toString()} ${total === 1 ? "server" : "servers"}`,
		`${running.toString()} running`,
		`${stopped.toString()} stopped`,
	];
	if (updates > 0) {
		parts.push(
			`${updates.toString()} ${updates === 1 ? "update" : "updates"} available`,
		);
	}
	return parts.join(" · ");
}
