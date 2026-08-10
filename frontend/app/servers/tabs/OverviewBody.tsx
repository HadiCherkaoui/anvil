// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useEffect, useState, type ReactElement } from "react";

import {
	ApiError,
	fetchServerMetrics,
	type ServerMetrics,
} from "../../lib/api";
import { useServerDetailCtx } from "../../lib/server-detail-context";
import { usePlayers } from "../../lib/use-players";

import { Card } from "../../components/Card";
import { CopyIcon } from "../../components/icons/Copy";
import { useToast } from "../../components/Toast";

const METRICS_POLL_MS = 5_000;

function formatLiveCpu(millicores: number | null): string {
	if (millicores === null) return "—";
	const cores = millicores / 1000;
	return `${cores.toFixed(2)} cores`;
}

function formatLiveMemory(memMi: number | null, limitMi: number): string {
	if (memMi === null) return "—";
	const pct =
		limitMi > 0 ? Math.min(100, Math.round((memMi / limitMi) * 100)) : 0;
	return `${memMi.toString()} / ${limitMi.toString()} MiB · ${pct.toString()}%`;
}

export function OverviewBody(): ReactElement {
	const detail = useServerDetailCtx();
	const toast = useToast();
	const isRunning = detail.status === "running";
	const players = usePlayers(detail.id, { enabled: isRunning });
	const [metrics, setMetrics] = useState<ServerMetrics | null>(null);
	const [metricsError, setMetricsError] = useState<string | null>(null);

	const onCopyAddr = (): void => {
		if (detail.endpoint === null) return;
		const addr = `${detail.endpoint.host}:${detail.endpoint.port.toString()}`;
		navigator.clipboard.writeText(addr).then(
			() => {
				toast.push("copied", "success");
			},
			() => {
				toast.push("clipboard unavailable", "error");
			},
		);
	};

	useEffect(() => {
		if (!isRunning) return undefined;
		const ctrl = new AbortController();
		let timer: number | null = null;
		const tick = (): void => {
			void fetchServerMetrics(detail.id, ctrl.signal)
				.then((m) => {
					setMetrics(m);
					setMetricsError(null);
				})
				.catch((err: unknown) => {
					if (err instanceof DOMException && err.name === "AbortError") return;
					const message =
						err instanceof ApiError
							? `${err.code}: ${err.message}`
							: err instanceof Error
								? err.message
								: "unknown error";
					setMetricsError(message);
				})
				.finally(() => {
					// Schedule only after the fetch settles — a reply slower
					// than the interval must not overlap the next tick.
					if (!ctrl.signal.aborted) {
						timer = window.setTimeout(tick, METRICS_POLL_MS);
					}
				});
		};
		tick();
		return () => {
			ctrl.abort();
			if (timer !== null) window.clearTimeout(timer);
		};
	}, [detail.id, isRunning]);

	const playerCountLabel = (): string => {
		if (!isRunning) return "—";
		if (players.status === "loading") return "loading…";
		if (players.data === null) return "—";
		return `${players.data.online.count.toString()} / ${players.data.online.max.toString()}`;
	};

	const playerNames = isRunning ? (players.data?.online.players ?? []) : [];

	return (
		<div className="grid gap-4 lg:grid-cols-2">
			<Card header="connection">
				<div className="flex items-center gap-2">
					<pre className="font-mono text-[12px] text-text-body">
						{detail.endpoint
							? `${detail.endpoint.host}:${detail.endpoint.port.toString()}`
							: "address pending…"}
					</pre>
					{detail.endpoint !== null && (
						<button
							type="button"
							onClick={onCopyAddr}
							aria-label="copy address"
							className="rounded p-1 text-text-faint hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						>
							<CopyIcon />
						</button>
					)}
				</div>
				<dl className="mt-4 grid grid-cols-[8rem_1fr] gap-y-1 font-mono text-[11px]">
					<dt className="text-text-muted">exposure</dt>
					<dd className="text-text-body">{detail.exposure_mode}</dd>
					{detail.nodeport !== null && (
						<>
							<dt className="text-text-muted">nodeport</dt>
							<dd className="text-text-body">{detail.nodeport}</dd>
						</>
					)}
					{detail.last_started_at !== null && (
						<>
							<dt className="text-text-muted">last started</dt>
							<dd className="text-text-body">
								{new Date(detail.last_started_at * 1000).toLocaleString()}
							</dd>
						</>
					)}
				</dl>
			</Card>

			<Card header="at a glance">
				<dl className="grid grid-cols-[8rem_1fr] gap-y-1 font-mono text-[12px]">
					<dt className="text-text-muted">runtime</dt>
					<dd className="text-text-body">{detail.source_kind}</dd>
					<dt className="text-text-muted">mc version</dt>
					<dd className="text-text-body">{detail.mc_version}</dd>
					<dt className="text-text-muted">memory limit</dt>
					<dd className="text-text-body">{detail.memory_mi} MiB</dd>
					<dt className="text-text-muted">storage</dt>
					<dd className="text-text-body">
						{detail.storage_size_gi} GiB
						{detail.storage_class !== null && (
							<span className="ml-1 text-text-muted">
								· {detail.storage_class}
							</span>
						)}
					</dd>
				</dl>
			</Card>

			<Card header="live · players">
				<dl className="grid grid-cols-[8rem_1fr] gap-y-1 font-mono text-[12px]">
					<dt className="text-text-muted">online</dt>
					<dd className="text-text-body">{playerCountLabel()}</dd>
				</dl>
				{playerNames.length > 0 && (
					<p className="mt-3 font-mono text-[11px] text-text-faint">
						{playerNames.join(" · ")}
					</p>
				)}
				{!isRunning && (
					<p className="mt-3 font-mono text-[11px] text-text-faint">
						start the server to see online players.
					</p>
				)}
				{isRunning &&
					players.status === "error" &&
					players.lastError !== null && (
						<p className="mt-3 font-mono text-[11px] text-state-error">
							players unavailable · {players.lastError}
						</p>
					)}
			</Card>

			<Card header="live · usage">
				<dl className="grid grid-cols-[8rem_1fr] gap-y-1 font-mono text-[12px]">
					<dt className="text-text-muted">cpu</dt>
					<dd className="text-text-body">
						{isRunning ? formatLiveCpu(metrics?.cpu_millicores ?? null) : "—"}
					</dd>
					<dt className="text-text-muted">memory</dt>
					<dd className="text-text-body">
						{isRunning
							? formatLiveMemory(metrics?.memory_mi ?? null, detail.memory_mi)
							: "—"}
					</dd>
				</dl>
				{isRunning &&
					metrics !== null &&
					metrics.cpu_millicores === null &&
					metrics.memory_mi === null && (
						<p className="mt-3 font-mono text-[11px] text-text-faint">
							metrics-server not installed (or no scrape yet) — values will
							appear once the metrics API is reachable.
						</p>
					)}
				{!isRunning && (
					<p className="mt-3 font-mono text-[11px] text-text-faint">
						start the server to see live cpu and memory.
					</p>
				)}
				{isRunning && metricsError !== null && (
					<p className="mt-3 font-mono text-[11px] text-state-error">
						metrics fetch failed · {metricsError}
					</p>
				)}
			</Card>
		</div>
	);
}
