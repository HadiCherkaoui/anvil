"use client";

import { useRouter } from "next/navigation";
import { useState, type KeyboardEvent, type ReactElement } from "react";

import {
	ApiError,
	restartServer,
	startServer,
	stopServer,
	type ServerSummary,
	type SourceKind,
} from "../lib/api";
import { cn } from "../lib/cn";

import { Badge, type BadgeVariant } from "./Badge";
import { Button } from "./Button";
import { Dropdown } from "./Dropdown";
import { useToast } from "./Toast";

interface ServerListProps {
	servers: readonly ServerSummary[];
	onActionDone: () => void;
}

function formatMemory(memoryMi: number): string {
	if (memoryMi % 1024 === 0) return `${(memoryMi / 1024).toString()} GiB`;
	return `${memoryMi.toString()} MiB`;
}

function formatCpu(millicores: number): string {
	return `${(millicores / 1000).toFixed(2)} cores`;
}

function formatEndpoint(endpoint: ServerSummary["endpoint"]): string {
	if (endpoint === null) return "—";
	return `${endpoint.host}:${endpoint.port.toString()}`;
}

const SOURCE_BAR: Record<SourceKind, string | null> = {
	vanilla: null,
	curseforge: "bg-source-curseforge",
	modrinth: "bg-source-modrinth",
	modded: "bg-source-modrinth",
	paper: "bg-source-local",
};

const STATUS_VARIANT: Record<ServerSummary["status"], BadgeVariant> = {
	running: "running",
	stopped: "stopped",
	starting: "starting",
	stopping: "stopping",
	error: "error",
};

export function ServerList({
	servers,
	onActionDone,
}: ServerListProps): ReactElement {
	if (servers.length === 0) {
		return <EmptyState />;
	}
	return (
		<div className="overflow-hidden rounded-md border border-border">
			<table className="w-full text-left font-mono text-[12px]">
				<thead className="border-b border-border-soft text-[10px] uppercase tracking-[0.10em] text-text-faint">
					<tr>
						<th className="py-3 pl-5 pr-4 font-medium">name</th>
						<th className="px-4 py-3 font-medium">status</th>
						<th className="px-4 py-3 font-medium">version</th>
						<th className="px-4 py-3 font-medium">cpu</th>
						<th className="px-4 py-3 font-medium">memory</th>
						<th className="px-4 py-3 font-medium">address</th>
						<th className="py-3 pl-4 pr-5" />
					</tr>
				</thead>
				<tbody className="divide-y divide-border-soft">
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
	const router = useRouter();
	const toast = useToast();
	const [busy, setBusy] = useState(false);

	const sourceBarClass = SOURCE_BAR[server.source_kind];
	const detailHref = `/servers?name=${encodeURIComponent(server.name)}`;

	const navigate = (): void => {
		router.push(detailHref);
	};

	const onRowKey = (event: KeyboardEvent<HTMLTableRowElement>): void => {
		if (event.key === "Enter" || event.key === " ") {
			event.preventDefault();
			navigate();
		}
	};

	const action = (label: string, fn: () => Promise<unknown>): void => {
		setBusy(true);
		fn()
			.then(() => {
				toast.push(`${server.name} · ${label} ok`, "success");
				onActionDone();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`${server.name} · ${label} failed: ${msg}`, "error");
			})
			.finally(() => {
				setBusy(false);
			});
	};

	const canStart = server.status === "stopped";
	const canStop = server.status === "running";
	const canRestart = server.status === "running";

	return (
		<tr
			tabIndex={0}
			onClick={navigate}
			onKeyDown={onRowKey}
			className="group cursor-pointer transition-colors hover:bg-elevated focus-visible:bg-elevated focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-accent"
		>
			<td className="relative py-3 pl-5 pr-4">
				{sourceBarClass !== null && (
					<span
						className={cn(
							"absolute left-0 top-1/2 h-3.5 w-1 -translate-y-1/2 rounded-r-sm",
							sourceBarClass,
						)}
						aria-hidden="true"
					/>
				)}
				<span className="text-text-primary">{server.name}</span>
				{server.update_available && (
					<span className="ml-2 text-accent" title="update available">
						↑1
					</span>
				)}
			</td>
			<td className="px-4 py-3">
				<Badge variant={STATUS_VARIANT[server.status]} />
			</td>
			<td className="px-4 py-3 text-text-body">{server.mc_version}</td>
			<td className="px-4 py-3 text-text-body">
				{formatCpu(server.cpu_millicores)}
			</td>
			<td className="px-4 py-3 text-text-body">
				{formatMemory(server.memory_mi)}
			</td>
			<td className="px-4 py-3 text-text-muted">
				{formatEndpoint(server.endpoint)}
			</td>
			<td
				className="py-3 pl-4 pr-5"
				onClick={(e) => {
					e.stopPropagation();
				}}
			>
				<div className="flex items-center justify-end gap-2 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
					{canStart && (
						<Button
							variant="primary"
							size="sm"
							disabled={busy}
							onClick={() => {
								action("start", () => startServer(server.id));
							}}
						>
							start
						</Button>
					)}
					{canStop && (
						<Button
							size="sm"
							disabled={busy}
							onClick={() => {
								action("stop", () => stopServer(server.id));
							}}
						>
							stop
						</Button>
					)}
					{canRestart && (
						<Button
							size="sm"
							disabled={busy}
							onClick={() => {
								action("restart", () => restartServer(server.id));
							}}
						>
							restart
						</Button>
					)}
					<Dropdown
						ariaLabel="more actions"
						trigger={<span aria-hidden>⋯</span>}
						items={[
							{
								id: "open",
								label: "open detail",
								onSelect: () => {
									router.push(detailHref);
								},
							},
							{
								id: "console",
								label: "open console",
								onSelect: () => {
									router.push(`${detailHref}&tab=console`);
								},
							},
						]}
					/>
				</div>
			</td>
		</tr>
	);
}

function EmptyState(): ReactElement {
	return (
		<div className="flex flex-col items-center gap-4 rounded-md border border-dashed border-border py-24 text-center">
			<svg
				aria-hidden
				className="text-text-faint"
				width={56}
				height={56}
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				strokeWidth={1.5}
			>
				<path d="M4 14l4-8h8l4 8M6 14h12v4H6z" />
			</svg>
			<h2 className="font-mono text-[14px] uppercase tracking-wider text-text-primary">
				no servers yet
			</h2>
			<p className="max-w-sm font-mono text-[12px] text-text-muted">
				click <span className="text-accent">[+ new]</span> to forge one.
			</p>
		</div>
	);
}
