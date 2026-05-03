"use client";

import { useState, type ReactElement } from "react";

import {
	ApiError,
	addPendingMod,
	applyMods,
	moddedConfigSchema,
	removePendingMod,
	type ModEntry,
	type ModPendingOp,
	type ModdedConfig,
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
			<Card>
				<p className="font-mono text-[12px] text-text-muted">
					paper plugin browsing arrives later. install plugins via FileBrowser
					at files.cherkaoui.ch for now.
				</p>
			</Card>
		);
	}

	if (
		detail.source_kind === "curseforge" ||
		detail.source_kind === "modrinth"
	) {
		return (
			<Card header={`bundled in ${detail.mc_version}`}>
				<p className="font-mono text-[12px] text-text-muted">
					pack-driven · changes get wiped at next pack update. view mods/ via
					FileBrowser at files.cherkaoui.ch for now.
				</p>
			</Card>
		);
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
