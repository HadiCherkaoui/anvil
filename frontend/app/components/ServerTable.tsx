"use client";

import Link from "next/link";
import { useState, type ReactElement } from "react";

import {
	ApiError,
	startServer,
	stopServer,
	type ServerSummary,
} from "../lib/api";
import { Button } from "./Button";
import { StatusBadge } from "./StatusBadge";

interface ServerTableProps {
	servers: readonly ServerSummary[];
	onActionDone: () => void;
}

function formatMemory(memoryMi: number): string {
	if (memoryMi % 1024 === 0) {
		const gi = memoryMi / 1024;
		return `${gi.toString()} GiB`;
	}
	return `${memoryMi.toString()} MiB`;
}

function formatEndpoint(endpoint: ServerSummary["endpoint"]): string {
	if (endpoint === null) return "—";
	return `${endpoint.host}:${endpoint.port.toString()}`;
}

export function ServerTable({
	servers,
	onActionDone,
}: ServerTableProps): ReactElement {
	if (servers.length === 0) {
		return <EmptyState />;
	}
	return (
		<div className="overflow-hidden rounded-lg border border-slate-800">
			<table className="w-full text-left text-sm">
				<thead className="bg-slate-900/60 text-xs uppercase tracking-wide text-slate-400">
					<tr>
						<th className="px-4 py-3">Name</th>
						<th className="px-4 py-3">Status</th>
						<th className="px-4 py-3">Version</th>
						<th className="px-4 py-3">Memory</th>
						<th className="px-4 py-3">Address</th>
						<th className="px-4 py-3 text-right">Actions</th>
					</tr>
				</thead>
				<tbody className="divide-y divide-slate-800">
					{servers.map((server) => (
						<ServerRow
							key={server.id}
							server={server}
							onActionDone={onActionDone}
						/>
					))}
				</tbody>
			</table>
		</div>
	);
}

interface ServerRowProps {
	server: ServerSummary;
	onActionDone: () => void;
}

function ServerRow({ server, onActionDone }: ServerRowProps): ReactElement {
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);

	const canStart = server.status === "stopped";
	const canStop = server.status === "running" || server.status === "starting";

	const action = (fn: () => Promise<unknown>): void => {
		setError(null);
		setBusy(true);
		fn()
			.then(() => {
				onActionDone();
			})
			.catch((err: unknown) => {
				if (err instanceof ApiError) {
					setError(`${err.code}: ${err.message}`);
				} else {
					setError(err instanceof Error ? err.message : "unknown error");
				}
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<tr className="hover:bg-slate-900/40">
			<td className="px-4 py-3 font-mono text-slate-100">{server.name}</td>
			<td className="px-4 py-3">
				<StatusBadge status={server.status} />
			</td>
			<td className="px-4 py-3 font-mono text-slate-300">
				{server.mc_version}
			</td>
			<td className="px-4 py-3 text-slate-300">
				{formatMemory(server.memory_mi)}
			</td>
			<td className="px-4 py-3 font-mono text-slate-300">
				{formatEndpoint(server.endpoint)}
			</td>
			<td className="px-4 py-3">
				<div className="flex items-center justify-end gap-2">
					{canStart && (
						<Button
							variant="primary"
							disabled={busy}
							onClick={() => {
								action(() => startServer(server.id));
							}}
						>
							start
						</Button>
					)}
					{canStop && (
						<Button
							variant="secondary"
							disabled={busy}
							onClick={() => {
								action(() => stopServer(server.id));
							}}
						>
							stop
						</Button>
					)}
					<Link
						href={{ pathname: "/servers/detail", query: { id: server.id } }}
						className="rounded-md bg-slate-700/40 px-3 py-1.5 text-sm font-medium text-slate-200 hover:bg-slate-700/60"
					>
						open
					</Link>
				</div>
				{error !== null && (
					<p className="mt-1 text-right text-xs text-red-400">{error}</p>
				)}
			</td>
		</tr>
	);
}

function EmptyState(): ReactElement {
	return (
		<div className="flex flex-col items-center gap-4 rounded-lg border border-dashed border-slate-800 py-24 text-center">
			<svg
				aria-hidden
				className="text-slate-600"
				width={48}
				height={48}
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				strokeWidth={2}
				strokeLinecap="round"
				strokeLinejoin="round"
			>
				<rect x="2" y="2" width="20" height="8" rx="2" />
				<rect x="2" y="14" width="20" height="8" rx="2" />
				<line x1="6" y1="6" x2="6.01" y2="6" />
				<line x1="6" y1="18" x2="6.01" y2="18" />
			</svg>
			<h2 className="text-lg font-medium">No servers yet.</h2>
			<p className="max-w-sm text-sm text-slate-400">
				Click <span className="font-mono">+ new server</span> to spin one up.
			</p>
		</div>
	);
}
