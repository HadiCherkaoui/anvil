"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";

import {
	ApiError,
	applyUpdate,
	changeServerVersion,
	deleteServer,
	fetchServerByName,
	restartServer,
	startServer,
	stopServer,
	type ServerDetail,
} from "../lib/api";
import { ServerDetailContext, type ServerDetailValue } from "../lib/server-detail-context";

import { Badge, type BadgeVariant } from "../components/Badge";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { ConfirmDeleteDialog } from "../components/ConfirmDeleteDialog";
import { Dropdown } from "../components/Dropdown";
import { Skeleton } from "../components/Skeleton";
import { Tabs, type Tab } from "../components/Tabs";
import { useToast } from "../components/Toast";
import { UpdateSheet } from "../components/UpdateSheet";

import { BackupsBody } from "./tabs/BackupsBody";
import { ConsoleBody } from "./tabs/ConsoleBody";
import { FilesBody } from "./tabs/FilesBody";
import { ModsBody } from "./tabs/ModsBody";
import { OverviewBody } from "./tabs/OverviewBody";
import { PlayersBody } from "./tabs/PlayersBody";
import { PropertiesBody } from "./tabs/PropertiesBody";
import { SettingsBody } from "./tabs/SettingsBody";

const POLL_INTERVAL_MS = 5_000;

type TabId =
	| "overview"
	| "console"
	| "mods"
	| "players"
	| "backups"
	| "files"
	| "properties"
	| "settings";

const TAB_IDS: ReadonlyArray<TabId> = [
	"overview",
	"console",
	"mods",
	"players",
	"backups",
	"files",
	"properties",
	"settings",
];

function isTabId(v: string): v is TabId {
	return (TAB_IDS as ReadonlyArray<string>).includes(v);
}

const STATUS_VARIANT: Record<ServerDetail["status"], BadgeVariant> = {
	running: "running",
	stopped: "stopped",
	starting: "starting",
	stopping: "stopping",
	error: "error",
};

type LoadState =
	| { kind: "missing-name" }
	| { kind: "loading" }
	| { kind: "ready"; detail: ServerDetail }
	| { kind: "not-found" }
	| { kind: "error"; message: string };

export function ServerDetailView(): ReactElement {
	const router = useRouter();
	const search = useSearchParams();
	const toast = useToast();
	const name = search.get("name");
	const tabParam = search.get("tab") ?? "overview";
	const tab: TabId = isTabId(tabParam) ? tabParam : "overview";

	const [state, setState] = useState<LoadState>(
		name === null ? { kind: "missing-name" } : { kind: "loading" },
	);
	const [sheetOpen, setSheetOpen] = useState(false);
	const [deleteOpen, setDeleteOpen] = useState(false);
	const ctrlRef = useRef<AbortController | null>(null);

	const reload = useCallback(
		async (n: string, signal: AbortSignal): Promise<void> => {
			try {
				const detail = await fetchServerByName(n, signal);
				setState({ kind: "ready", detail });
			} catch (err: unknown) {
				if (err instanceof DOMException && err.name === "AbortError") return;
				if (err instanceof ApiError && err.status === 404) {
					setState({ kind: "not-found" });
					return;
				}
				const message =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				setState({ kind: "error", message });
			}
		},
		[],
	);

	const refresh = useCallback((): void => {
		if (name === null) return;
		const ctrl = ctrlRef.current;
		if (ctrl === null) return;
		void reload(name, ctrl.signal);
	}, [name, reload]);

	useEffect(() => {
		if (name === null) return undefined;
		const ctrl = new AbortController();
		ctrlRef.current = ctrl;
		let timer: number | undefined;
		const tick = (): void => {
			if (document.visibilityState === "visible") {
				void reload(name, ctrl.signal);
			}
			timer = window.setTimeout(tick, POLL_INTERVAL_MS);
		};
		tick();
		const onVis = (): void => {
			if (document.visibilityState === "visible")
				void reload(name, ctrl.signal);
		};
		document.addEventListener("visibilitychange", onVis);
		return () => {
			if (timer !== undefined) window.clearTimeout(timer);
			document.removeEventListener("visibilitychange", onVis);
			ctrl.abort();
			if (ctrlRef.current === ctrl) ctrlRef.current = null;
		};
	}, [name, reload]);

	if (state.kind === "missing-name") {
		return (
			<main className="px-5 py-6">
				<Card>
					<p className="font-mono text-[12px] text-state-error">
						missing ?name= in URL
					</p>
				</Card>
			</main>
		);
	}
	if (state.kind === "loading") {
		return (
			<main className="px-5 py-6">
				<Skeleton variant="block" className="h-24" />
			</main>
		);
	}
	if (state.kind === "not-found") {
		return (
			<main className="px-5 py-6">
				<Card>
					<p className="font-mono text-[12px] text-state-error">
						server &quot;{name}&quot; not found
					</p>
				</Card>
			</main>
		);
	}
	if (state.kind === "error") {
		return (
			<main className="px-5 py-6">
				<Card>
					<p className="font-mono text-[12px] text-state-error">
						failed to load · {state.message}
					</p>
				</Card>
			</main>
		);
	}

	const detail = state.detail;
	const safeName = encodeURIComponent(detail.name);
	const tabHref = (id: TabId): string =>
		id === "overview"
			? `/servers?name=${safeName}`
			: `/servers?name=${safeName}&tab=${id}`;
	const tabs: ReadonlyArray<Tab> = [
		{ id: "overview", label: "overview", href: tabHref("overview") },
		{ id: "console", label: "console", href: tabHref("console") },
		{
			id: "mods",
			label: detail.source_kind === "paper" ? "plugins" : "mods",
			href: tabHref("mods"),
			...(detail.update_available || detail.mod_updates.length > 0
				? { mark: true }
				: {}),
		},
		{ id: "players", label: "players", href: tabHref("players") },
		{ id: "backups", label: "backups", href: tabHref("backups") },
		{ id: "files", label: "files", href: tabHref("files") },
		{ id: "properties", label: "properties", href: tabHref("properties") },
		{ id: "settings", label: "settings", href: tabHref("settings") },
	];

	const lifecycle = (label: string, fn: () => Promise<unknown>) => (): void => {
		fn()
			.then(() => {
				toast.push(`${detail.name} · ${label} ok`, "success");
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`${detail.name} · ${label} failed: ${msg}`, "error");
			});
	};

	const onUpdateClick = (): void => {
		setSheetOpen(true);
		if (!detail.update_in_progress) {
			applyUpdate(detail.id).catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`update failed to start: ${msg}`, "error");
			});
		}
	};

	const onLoaderUpdateClick = (): void => {
		if (detail.loader_update === null || detail.update_in_progress) return;
		setSheetOpen(true);
		changeServerVersion(detail.id, {
			mc_version: detail.mc_version,
			loader_version: detail.loader_update.latest_loader,
		}).catch((err: unknown) => {
			const msg =
				err instanceof ApiError
					? `${err.code}: ${err.message}`
					: err instanceof Error
						? err.message
						: "unknown error";
			toast.push(`loader update failed to start: ${msg}`, "error");
		});
	};

	const ctxValue: ServerDetailValue = { detail, refresh };

	return (
		<ServerDetailContext.Provider value={ctxValue}>
			<main className="px-5 py-6">
				<header className="mb-4 flex items-start justify-between gap-4">
					<div>
						<h1 className="font-mono text-[24px] font-semibold tracking-tight text-text-primary">
							{detail.name}
						</h1>
						<div className="mt-2 flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-[12px] text-text-muted">
							<Badge variant={STATUS_VARIANT[detail.status]} />
							<span>runtime · {detail.source_kind}</span>
							<span>version · {detail.mc_version}</span>
							<span>memory · {detail.memory_mi} MiB</span>
							<span>storage · {detail.storage_size_gi} GiB</span>
						</div>
					</div>
					<div className="flex items-center gap-2">
						{detail.status === "stopped" && (
							<Button
								variant="primary"
								onClick={lifecycle("start", () => startServer(detail.id))}
							>
								start
							</Button>
						)}
						{(detail.status === "running" ||
							detail.status === "starting" ||
							detail.status === "error") && (
							<Button onClick={lifecycle("stop", () => stopServer(detail.id))}>
								stop
							</Button>
						)}
						{detail.status === "running" && (
							<Button
								onClick={lifecycle("restart", () => restartServer(detail.id))}
							>
								restart
							</Button>
						)}
						<Dropdown
							ariaLabel="more actions"
							trigger={<span aria-hidden>⋯</span>}
							items={[
								{
									id: "console",
									label: "open console",
									onSelect: () => {
										router.push(tabHref("console"));
									},
								},
								{
									id: "settings",
									label: "open settings",
									onSelect: () => {
										router.push(tabHref("settings"));
									},
								},
								{
									id: "delete",
									label: "delete server",
									danger: true,
									onSelect: () => {
										setDeleteOpen(true);
									},
								},
							]}
						/>
					</div>
				</header>

				{detail.update_available && !detail.update_in_progress && (
					<div className="mb-4 flex items-center justify-between rounded-md border border-accent-border bg-accent-bg/30 px-4 py-3">
						<span className="font-mono text-[12px] text-accent">
							update available · {detail.mc_version} →{" "}
							{detail.latest_version_name ?? "?"}
						</span>
						<div className="flex gap-2">
							<Button variant="ghost">skip</Button>
							<Button variant="primary" onClick={onUpdateClick}>
								update
							</Button>
						</div>
					</div>
				)}

				{detail.loader_update !== null && !detail.update_in_progress && (
					<div className="mb-4 flex items-center justify-between rounded-md border border-state-warning/40 bg-state-warning/5 px-4 py-3">
						<span className="font-mono text-[12px] text-state-warning">
							loader update · {detail.loader_update.current_loader} →{" "}
							{detail.loader_update.latest_loader}
						</span>
						<Button variant="primary" onClick={onLoaderUpdateClick}>
							update loader
						</Button>
					</div>
				)}

				<Tabs tabs={tabs} activeId={tab} />
				<div className="mt-6">
					{tab === "overview" && <OverviewBody />}
					{tab === "console" && <ConsoleBody />}
					{tab === "mods" && <ModsBody />}
					{tab === "players" && <PlayersBody />}
					{tab === "backups" && <BackupsBody />}
					{tab === "files" && <FilesBody />}
					{tab === "properties" && <PropertiesBody />}
					{tab === "settings" && <SettingsBody />}
				</div>

				<UpdateSheet
					serverId={detail.id}
					isOpen={sheetOpen}
					onClose={() => {
						setSheetOpen(false);
					}}
				/>
				<ConfirmDeleteDialog
					open={deleteOpen}
					onClose={() => {
						setDeleteOpen(false);
					}}
					targetName={detail.name}
					description={
						detail.status === "stopped"
							? "this permanently removes the StatefulSet, PVC, Service, and RCON Secret. the server's world data is lost."
							: "stop the server first — running servers can't be deleted. this dialog will report a 409 if you try."
					}
					onConfirm={async () => {
						await deleteServer(detail.id);
						toast.push(`${detail.name} deleted`, "success");
						router.push("/");
					}}
				/>
			</main>
		</ServerDetailContext.Provider>
	);
}
