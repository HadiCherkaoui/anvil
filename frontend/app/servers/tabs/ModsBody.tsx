"use client";

import { useEffect, useState, type ReactElement } from "react";

import {
	ApiError,
	addPendingMod,
	addServerPlugin,
	applyMods,
	applyServerPlugins,
	fetchFileList,
	listServerPlugins,
	moddedConfigSchema,
	paperConfigSchema,
	removePendingMod,
	removeServerPlugin,
	type FileEntry,
	type ModEntry,
	type ModPendingOp,
	type ModdedConfig,
	type ServerDetail,
} from "../../lib/api";
import { useServerDetailCtx } from "../../lib/server-detail-context";

import { ApplySheet } from "../../components/ApplySheet";
import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { CatalogSheet, type CatalogPick } from "../../components/CatalogSheet";
import { useToast } from "../../components/Toast";

export function ModsBody(): ReactElement {
	const detail = useServerDetailCtx();
	const toast = useToast();
	const [browseOpen, setBrowseOpen] = useState(false);
	const [applyOpen, setApplyOpen] = useState(false);

	if (detail.source_kind === "vanilla") {
		return (
			<Card>
				<p className="font-mono text-[12px] text-text-muted">
					vanilla servers don&apos;t support mods.
				</p>
			</Card>
		);
	}

	if (detail.source_kind === "paper") {
		return (
			<PaperPluginsBody
				serverId={detail.id}
				sourceConfig={detail.source_config}
				browseOpen={browseOpen}
				setBrowseOpen={setBrowseOpen}
				applyOpen={applyOpen}
				setApplyOpen={setApplyOpen}
				onToast={(msg, kind) => {
					toast.push(msg, kind);
				}}
			/>
		);
	}

	if (
		detail.source_kind === "curseforge" ||
		detail.source_kind === "modrinth"
	) {
		return <PackModsBody serverId={detail.id} status={detail.status} />;
	}

	// modded
	const cfgParse = moddedConfigSchema.safeParse(detail.source_config);
	if (!cfgParse.success) {
		return (
			<Card>
				<p className="font-mono text-[12px] text-state-error">
					source_config did not parse as a modded config
				</p>
			</Card>
		);
	}
	const cfg: ModdedConfig = cfgParse.data;

	const onPick = (pick: CatalogPick): void => {
		const entry: ModEntry = {
			provider: pick.hit.provider,
			project_id: pick.hit.project_id,
			project_slug: pick.hit.slug,
			project_name: pick.hit.name,
			version_id: pick.version.version_id,
			version_name: pick.version.version_name,
			filename: pick.version.primary_filename,
			download_url: pick.version.primary_url,
			sha512: pick.version.primary_sha512,
		};
		const op: ModPendingOp = { op: "add", mod_entry: entry };
		addPendingMod(detail.id, op)
			.then(() => {
				toast.push(`queued · ${entry.project_name}`, "success");
			})
			.catch((err: unknown) => {
				toast.push(
					`queue failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	const removeInstalled = (filename: string): void => {
		const op: ModPendingOp = { op: "remove", filename };
		addPendingMod(detail.id, op)
			.then(() => {
				toast.push(`queued removal · ${filename}`, "success");
			})
			.catch((err: unknown) => {
				toast.push(
					`queue failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	const discardPending = (idx: number): void => {
		removePendingMod(detail.id, idx)
			.then(() => {
				toast.push("discarded", "success");
			})
			.catch((err: unknown) => {
				toast.push(
					`discard failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	const onApply = (): void => {
		applyMods(detail.id)
			.then(() => {
				setApplyOpen(true);
			})
			.catch((err: unknown) => {
				toast.push(
					`apply failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	return (
		<>
			<Card>
				<div className="flex items-baseline justify-between">
					<p className="font-mono text-[13px] text-text-primary">
						{cfg.mods.length} installed
						{cfg.pending.length > 0 && (
							<span className="ml-3 text-state-warning">
								· {cfg.pending.length} pending
							</span>
						)}
					</p>
					<Button
						onClick={() => {
							setBrowseOpen(true);
						}}
					>
						+ add mods
					</Button>
				</div>

				<ul className="mt-4 flex flex-col">
					{cfg.mods.length === 0 && (
						<li className="py-2 font-mono text-[12px] text-text-faint">
							no mods installed yet — click `+ add mods` to start.
						</li>
					)}
					{cfg.mods.map((m) => (
						<li
							key={m.filename}
							className="group flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
						>
							<div className="flex items-center gap-3">
								<span
									className="h-3.5 w-1 rounded-sm"
									style={{
										background:
											m.provider === "modrinth"
												? "var(--color-source-modrinth)"
												: "var(--color-source-curseforge)",
									}}
								/>
								<span className="text-text-body">{m.project_name}</span>
								<span className="text-text-faint">{m.version_name}</span>
							</div>
							<button
								type="button"
								onClick={() => {
									removeInstalled(m.filename);
								}}
								className="opacity-0 transition-opacity hover:text-state-error focus-visible:opacity-100 group-hover:opacity-100"
							>
								remove
							</button>
						</li>
					))}
				</ul>

				{cfg.pending.length > 0 && (
					<>
						<p className="mt-6 font-mono text-[11px] uppercase tracking-wider text-text-muted">
							pending changes
						</p>
						<ul className="mt-2 flex flex-col">
							{cfg.pending.map((op, i) => (
								<li
									key={`${op.op}-${i.toString()}`}
									className="group flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
								>
									<PendingLabel op={op} />
									<button
										type="button"
										onClick={() => {
											discardPending(i);
										}}
										className="opacity-0 transition-opacity hover:text-state-error focus-visible:opacity-100 group-hover:opacity-100"
									>
										discard
									</button>
								</li>
							))}
						</ul>
						<div className="mt-4 flex justify-end gap-2">
							<Button onClick={onApply} variant="primary">
								apply now
							</Button>
						</div>
					</>
				)}
			</Card>

			<CatalogSheet
				isOpen={browseOpen}
				onClose={() => {
					setBrowseOpen(false);
				}}
				mode="mod"
				loader={cfg.runtime}
				mc={cfg.mc_version}
				onPick={onPick}
			/>
			<ApplySheet
				serverId={detail.id}
				isOpen={applyOpen}
				onClose={() => {
					setApplyOpen(false);
				}}
			/>
		</>
	);
}

function PendingLabel({ op }: { op: ModPendingOp }): ReactElement {
	if (op.op === "add") {
		return (
			<span>
				<span className="mr-2 text-state-running">+</span>
				add · {op.mod_entry.project_name} {op.mod_entry.version_name}
			</span>
		);
	}
	if (op.op === "remove") {
		return (
			<span>
				<span className="mr-2 text-state-error">−</span>
				remove · {op.filename}
			</span>
		);
	}
	return (
		<span>
			<span className="mr-2 text-accent">↑</span>
			bump · {op.filename} → {op.to_version_name}
		</span>
	);
}

interface PaperPluginsProps {
	serverId: string;
	sourceConfig: unknown;
	browseOpen: boolean;
	setBrowseOpen: (v: boolean) => void;
	applyOpen: boolean;
	setApplyOpen: (v: boolean) => void;
	onToast: (msg: string, kind: "success" | "error") => void;
}

interface PendingPluginChange {
	kind: "add" | "remove";
	filename: string;
	label: string;
}

function diffPending(
	plugins: readonly ModEntry[],
	pending: readonly ModEntry[],
): PendingPluginChange[] {
	if (pending.length === 0) return [];
	const installedByName = new Map(plugins.map((p) => [p.filename, p]));
	const desiredByName = new Map(pending.map((p) => [p.filename, p]));
	const changes: PendingPluginChange[] = [];
	for (const p of pending) {
		if (!installedByName.has(p.filename)) {
			changes.push({
				kind: "add",
				filename: p.filename,
				label: `${p.project_name} ${p.version_name}`,
			});
		}
	}
	for (const p of plugins) {
		if (!desiredByName.has(p.filename)) {
			changes.push({
				kind: "remove",
				filename: p.filename,
				label: p.filename,
			});
		}
	}
	return changes;
}

function PaperPluginsBody({
	serverId,
	sourceConfig,
	browseOpen,
	setBrowseOpen,
	applyOpen,
	setApplyOpen,
	onToast,
}: PaperPluginsProps): ReactElement {
	const cfgParse = paperConfigSchema.safeParse(sourceConfig);
	const initialPlugins = cfgParse.success ? cfgParse.data.plugins : [];
	const initialPending = cfgParse.success ? cfgParse.data.pending_plugins : [];

	const [plugins, setPlugins] = useState<readonly ModEntry[]>(initialPlugins);
	const [pending, setPending] = useState<readonly ModEntry[]>(initialPending);
	const [mcVersion] = useState<string>(
		cfgParse.success ? cfgParse.data.mc_version : "",
	);

	useEffect(() => {
		const ctrl = new AbortController();
		listServerPlugins(serverId, ctrl.signal)
			.then((r) => {
				setPlugins(r.plugins);
				setPending(r.pending_plugins);
			})
			.catch((err: unknown) => {
				if (err instanceof DOMException && err.name === "AbortError") return;
				onToast(
					`load failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
		return () => {
			ctrl.abort();
		};
	}, [serverId, onToast]);

	if (!cfgParse.success) {
		return (
			<Card>
				<p className="font-mono text-[12px] text-state-error">
					source_config did not parse as a paper config
				</p>
			</Card>
		);
	}

	const changes = diffPending(plugins, pending);

	const refresh = (): void => {
		const ctrl = new AbortController();
		listServerPlugins(serverId, ctrl.signal)
			.then((r) => {
				setPlugins(r.plugins);
				setPending(r.pending_plugins);
			})
			.catch((err: unknown) => {
				if (err instanceof DOMException && err.name === "AbortError") return;
				onToast(
					`refresh failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	const onPick = (pick: CatalogPick): void => {
		const entry: ModEntry = {
			provider: pick.hit.provider,
			project_id: pick.hit.project_id,
			project_slug: pick.hit.slug,
			project_name: pick.hit.name,
			version_id: pick.version.version_id,
			version_name: pick.version.version_name,
			filename: pick.version.primary_filename,
			download_url: pick.version.primary_url,
			sha512: pick.version.primary_sha512,
		};
		addServerPlugin(serverId, entry)
			.then(() => {
				onToast(`queued · ${entry.project_name}`, "success");
				refresh();
			})
			.catch((err: unknown) => {
				onToast(
					`queue failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	const removeInstalled = (filename: string): void => {
		removeServerPlugin(serverId, filename)
			.then(() => {
				onToast(`queued removal · ${filename}`, "success");
				refresh();
			})
			.catch((err: unknown) => {
				onToast(
					`queue failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	const discardChange = (change: PendingPluginChange): void => {
		if (change.kind === "add") {
			removeServerPlugin(serverId, change.filename)
				.then(() => {
					onToast("discarded", "success");
					refresh();
				})
				.catch((err: unknown) => {
					onToast(
						`discard failed · ${
							err instanceof ApiError ? err.code : "unknown"
						}`,
						"error",
					);
				});
			return;
		}
		const original = plugins.find((p) => p.filename === change.filename);
		if (!original) return;
		addServerPlugin(serverId, original)
			.then(() => {
				onToast("discarded", "success");
				refresh();
			})
			.catch((err: unknown) => {
				onToast(
					`discard failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	const onApply = (): void => {
		applyServerPlugins(serverId)
			.then(() => {
				setApplyOpen(true);
			})
			.catch((err: unknown) => {
				onToast(
					`apply failed · ${err instanceof ApiError ? err.code : "unknown"}`,
					"error",
				);
			});
	};

	return (
		<>
			<Card>
				<div className="flex items-baseline justify-between">
					<p className="font-mono text-[13px] text-text-primary">
						{plugins.length} installed
						{changes.length > 0 && (
							<span className="ml-3 text-state-warning">
								· {changes.length} pending
							</span>
						)}
					</p>
					<Button
						onClick={() => {
							setBrowseOpen(true);
						}}
					>
						+ add plugins
					</Button>
				</div>

				<ul className="mt-4 flex flex-col">
					{plugins.length === 0 && (
						<li className="py-2 font-mono text-[12px] text-text-faint">
							no plugins installed yet — click `+ add plugins` to start.
						</li>
					)}
					{plugins.map((p) => (
						<li
							key={p.filename}
							className="group flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
						>
							<div className="flex items-center gap-3">
								<span
									className="h-3.5 w-1 rounded-sm"
									style={{ background: "var(--color-source-modrinth)" }}
								/>
								<span className="text-text-body">{p.project_name}</span>
								<span className="text-text-faint">{p.version_name}</span>
							</div>
							<button
								type="button"
								onClick={() => {
									removeInstalled(p.filename);
								}}
								className="opacity-0 transition-opacity hover:text-state-error focus-visible:opacity-100 group-hover:opacity-100"
							>
								remove
							</button>
						</li>
					))}
				</ul>

				{changes.length > 0 && (
					<>
						<p className="mt-6 font-mono text-[11px] uppercase tracking-wider text-text-muted">
							pending changes
						</p>
						<ul className="mt-2 flex flex-col">
							{changes.map((c) => (
								<li
									key={`${c.kind}-${c.filename}`}
									className="group flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
								>
									<span>
										<span
											className={
												c.kind === "add"
													? "mr-2 text-state-running"
													: "mr-2 text-state-error"
											}
										>
											{c.kind === "add" ? "+" : "−"}
										</span>
										{c.kind === "add" ? "add" : "remove"} · {c.label}
									</span>
									<button
										type="button"
										onClick={() => {
											discardChange(c);
										}}
										className="opacity-0 transition-opacity hover:text-state-error focus-visible:opacity-100 group-hover:opacity-100"
									>
										discard
									</button>
								</li>
							))}
						</ul>
						<div className="mt-4 flex justify-end gap-2">
							<Button onClick={onApply} variant="primary">
								apply now
							</Button>
						</div>
					</>
				)}
			</Card>

			<CatalogSheet
				isOpen={browseOpen}
				onClose={() => {
					setBrowseOpen(false);
				}}
				mode="plugin"
				loader="paper"
				mc={mcVersion}
				onPick={onPick}
			/>
			<ApplySheet
				serverId={serverId}
				isOpen={applyOpen}
				onClose={() => {
					setApplyOpen(false);
				}}
				target="plugins"
			/>
		</>
	);
}

interface PackModsBodyProps {
	serverId: string;
	status: ServerDetail["status"];
}

type PackModsState =
	| { kind: "loading" }
	| { kind: "ready"; entries: readonly FileEntry[] }
	| { kind: "pvc-uninit" }
	| { kind: "error"; message: string };

function transientReasonForStatus(
	status: ServerDetail["status"],
): string | null {
	if (status === "starting" || status === "stopping") {
		return `server is ${status} — try again once it settles.`;
	}
	if (status === "error") {
		return "server is in error state — start it or fix the failure first.";
	}
	return null;
}

function formatJarSize(bytes: number): string {
	if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
	if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KiB`;
	return `${bytes.toString()} B`;
}

// Strips `.jar` and tries to peel off a trailing version suffix so the
// list reads like "JEI 15.3.0.4" instead of "jei-1.20.1-15.3.0.4.jar".
function prettifyJarName(filename: string): { name: string; version: string } {
	const stem = filename.replace(/\.jar$/i, "");
	const versionMatch = /^(.*?)[-_]([0-9].*)$/.exec(stem);
	if (versionMatch && versionMatch[1] && versionMatch[2]) {
		return { name: versionMatch[1], version: versionMatch[2] };
	}
	return { name: stem, version: "" };
}

function PackModsBody({ serverId, status }: PackModsBodyProps): ReactElement {
	const transientReason = transientReasonForStatus(status);
	const [state, setState] = useState<PackModsState>({ kind: "loading" });

	useEffect(() => {
		// Render-time gate: while the server is in a transient/error state, the
		// effect bails and the card shows the reason directly.
		if (transientReason !== null) return undefined;
		const ctrl = new AbortController();
		fetchFileList(serverId, "/mods", ctrl.signal)
			.then((res) => {
				const jars = res.entries
					.filter((e) => e.type === "f" && /\.jar$/i.test(e.name))
					.toSorted((a, b) => a.name.localeCompare(b.name));
				setState({ kind: "ready", entries: jars });
			})
			.catch((err: unknown) => {
				if (err instanceof DOMException && err.name === "AbortError") return;
				if (err instanceof ApiError && err.status === 404) {
					setState({ kind: "ready", entries: [] });
					return;
				}
				if (err instanceof ApiError && err.code === "pvc_not_initialized") {
					setState({ kind: "pvc-uninit" });
					return;
				}
				const message =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				setState({ kind: "error", message });
			});
		return () => {
			ctrl.abort();
		};
	}, [serverId, transientReason]);

	if (transientReason !== null) {
		return (
			<Card header="installed mods · pack-driven">
				<p className="font-mono text-[12px] text-text-muted">
					{transientReason}
				</p>
			</Card>
		);
	}

	return (
		<Card header="installed mods · pack-driven">
			{state.kind === "loading" && (
				<p className="font-mono text-[12px] text-text-faint">
					loading mod list…
				</p>
			)}
			{state.kind === "pvc-uninit" && (
				<p className="font-mono text-[12px] text-text-muted">
					start the server once to initialise storage, then come back.
				</p>
			)}
			{state.kind === "error" && (
				<p className="font-mono text-[12px] text-state-error">
					failed · {state.message}
				</p>
			)}
			{state.kind === "ready" && state.entries.length === 0 && (
				<p className="font-mono text-[12px] text-text-faint">
					no jars in /mods.
				</p>
			)}
			{state.kind === "ready" && state.entries.length > 0 && (
				<>
					<p className="mb-3 font-mono text-[11px] text-text-faint">
						{state.entries.length.toString()} jars · pack-driven · changes get
						wiped on the next pack update.
					</p>
					<ul className="flex flex-col">
						{state.entries.map((m) => {
							const { name, version } = prettifyJarName(m.name);
							return (
								<li
									key={m.name}
									className="flex items-center justify-between border-b border-border-soft py-1.5 font-mono text-[12px]"
								>
									<div className="flex min-w-0 items-center gap-3">
										<span className="truncate text-text-body">{name}</span>
										{version !== "" && (
											<span className="shrink-0 text-text-faint">
												{version}
											</span>
										)}
									</div>
									<span className="shrink-0 text-text-faint">
										{formatJarSize(m.size)}
									</span>
								</li>
							);
						})}
					</ul>
				</>
			)}
		</Card>
	);
}
