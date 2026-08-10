// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

"use client";

import { useRouter } from "next/navigation";
import { useEffect, useState, type ReactElement } from "react";

import {
	ApiError,
	deleteServer,
	fetchCapabilities,
	resizeServerStorage,
	updateServerSettings,
	type AutoUpdateMode,
	type ClusterCapabilities,
} from "../../lib/api";
import { useMcVersions } from "../../lib/use-mc-versions";
import { useServerDetail } from "../../lib/server-detail-context";

import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { ConfirmDeleteDialog } from "../../components/ConfirmDeleteDialog";
import { RangeSlider } from "../../components/RangeSlider";
import { SegmentedControl } from "../../components/SegmentedControl";
import { useToast } from "../../components/Toast";
import { VersionChangeSheet } from "../../components/VersionChangeSheet";

const AUTO_UPDATE_OPTIONS: ReadonlyArray<{
	value: AutoUpdateMode;
	label: string;
}> = [
	{ value: "never", label: "never" },
	{ value: "notify", label: "notify" },
	{ value: "apply", label: "apply" },
];

function readAutoUpdate(
	detail: ReturnType<typeof useServerDetail>["detail"],
): AutoUpdateMode {
	if (detail.source_kind === "vanilla") return "notify";
	const cfg = detail.source_config;
	if (cfg !== null && typeof cfg === "object" && "auto_update_mode" in cfg) {
		const v = (cfg as { auto_update_mode?: unknown }).auto_update_mode;
		if (v === "never" || v === "notify" || v === "apply") return v;
	}
	return "notify";
}

export function SettingsBody(): ReactElement {
	const { detail, refresh } = useServerDetail();
	const router = useRouter();
	const toast = useToast();
	const versions = useMcVersions();

	const [memory, setMemory] = useState(detail.memory_mi);
	const [autoUpdate, setAutoUpdate] = useState<AutoUpdateMode>(
		readAutoUpdate(detail),
	);
	const [busy, setBusy] = useState(false);
	const [confirmOpen, setConfirmOpen] = useState(false);
	const [caps, setCaps] = useState<ClusterCapabilities | null>(null);
	const [pendingSize, setPendingSize] = useState<number>(detail.storage_size_gi);
	const [resizing, setResizing] = useState(false);
	const [versionSheetOpen, setVersionSheetOpen] = useState(false);

	useEffect(() => {
		const ctrl = new AbortController();
		fetchCapabilities(ctrl.signal)
			.then((c) => {
				setCaps(c);
			})
			.catch(() => {
				// non-fatal: storage card stays hidden if caps fail
			});
		return (): void => {
			ctrl.abort();
		};
	}, []);

	// `auto_update_mode` lives in `source_config` for every non-vanilla
	// type. Semantics differ by kind:
	//   • curseforge / modrinth → modpack-level auto apply (orchestrator).
	//   • modded / paper → per-mod / per-plugin auto-update (poller +
	//     sync FSM, see backend `poll_individual_mods`).
	const supportsAutoUpdate = detail.source_kind !== "vanilla";
	const autoUpdateLabel =
		detail.source_kind === "curseforge" || detail.source_kind === "modrinth"
			? "modpack auto-update"
			: detail.source_kind === "paper"
				? "plugin auto-update"
				: "mod auto-update";
	const isVersionChangeable =
		detail.source_kind !== "curseforge" && detail.source_kind !== "modrinth";
	const moddedLoader = ((): { runtime: string; version: string | null } | null => {
		if (detail.source_kind !== "modded") return null;
		const cfg = detail.source_config;
		if (cfg === null || typeof cfg !== "object") return null;
		const r = (cfg as { runtime?: unknown }).runtime;
		if (r !== "forge" && r !== "neoforge") return null;
		const v = (cfg as { loader_version?: unknown }).loader_version;
		return { runtime: r, version: typeof v === "string" ? v : null };
	})();

	const sc = detail.storage_class ?? caps?.default_storage_class ?? "";
	const canExpand =
		caps !== null && sc !== "" && caps.expandable_storage_classes.includes(sc);
	const storageMax = Math.max(
		detail.storage_size_gi * 4,
		detail.storage_size_gi + 10,
	);

	const onExpand = (): void => {
		if (pendingSize <= detail.storage_size_gi) return;
		setResizing(true);
		void resizeServerStorage(detail.id, pendingSize)
			.then(() => {
				toast.push("resize requested", "success");
				refresh();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`resize failed · ${msg}`, "error");
			})
			.finally(() => {
				setResizing(false);
			});
	};

	const memoryDirty = memory !== detail.memory_mi;
	const autoUpdateDirty =
		supportsAutoUpdate && autoUpdate !== readAutoUpdate(detail);
	const dirty = memoryDirty || autoUpdateDirty;

	const save = (): void => {
		setBusy(true);
		const patch = {
			...(memoryDirty ? { memory_mi: memory } : {}),
			...(autoUpdateDirty ? { auto_update_mode: autoUpdate } : {}),
		};
		void updateServerSettings(detail.id, patch)
			.then(() => {
				toast.push("settings saved · applies on next start", "success");
				// Without this the context keeps the pre-save detail and the
				// form still reads dirty until the next 5 s poll.
				refresh();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`save failed: ${msg}`, "error");
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<div className="flex max-w-2xl flex-col gap-4">
			<Card header="resources · apply on next start">
				<div className="flex flex-col gap-4">
					<RangeSlider
						label="memory"
						value={memory}
						onChange={setMemory}
						min={1024}
						max={65536}
						step={1024}
						unit="MiB"
					/>
				</div>
			</Card>

			{canExpand && (
				<Card header="storage · grow only">
					<div className="flex flex-col gap-3">
						<p className="font-mono text-[12px] text-text-muted">
							current · {detail.storage_size_gi} GiB
						</p>
						<RangeSlider
							label="resize to"
							value={pendingSize}
							onChange={setPendingSize}
							min={detail.storage_size_gi}
							max={storageMax}
							step={1}
							unit="GiB"
						/>
						<div className="flex items-center justify-between gap-3">
							<p className="font-mono text-[11px] text-text-faint">
								shrink not supported · CSI expansion is asynchronous
							</p>
							<Button
								variant="primary"
								size="sm"
								onClick={onExpand}
								disabled={pendingSize <= detail.storage_size_gi || resizing}
							>
								expand to {pendingSize} GiB
							</Button>
						</div>
					</div>
				</Card>
			)}

			{isVersionChangeable && (
				<Card header="version">
					<div className="flex flex-col gap-2 font-mono text-[12px]">
						<div className="flex items-baseline justify-between gap-3">
							<div className="grid grid-cols-[80px_1fr] items-baseline gap-3">
								<span className="text-[11px] uppercase tracking-wider text-text-muted">
									mc
								</span>
								<span className="text-text-body">{detail.mc_version}</span>
							</div>
							<Button
								variant="secondary"
								size="sm"
								onClick={() => {
									setVersionSheetOpen(true);
								}}
							>
								edit
							</Button>
						</div>
						{moddedLoader !== null && (
							<div className="grid grid-cols-[80px_1fr] items-baseline gap-3">
								<span className="text-[11px] uppercase tracking-wider text-text-muted">
									{moddedLoader.runtime}
								</span>
								<span className="text-text-body">
									{moddedLoader.version ?? "—"}
								</span>
							</div>
						)}
						{versions !== undefined && versions.versions.length > 0 && (
							<p className="mt-1 text-[11px] text-text-faint">
								upstream releases · {versions.versions.slice(0, 8).join(", ")}
								{versions.versions.length > 8 && " …"}
							</p>
						)}
					</div>
				</Card>
			)}

			<VersionChangeSheet
				isOpen={versionSheetOpen}
				onClose={() => {
					setVersionSheetOpen(false);
				}}
				detail={detail}
			/>

			{supportsAutoUpdate && (
				<Card header={autoUpdateLabel}>
					<SegmentedControl
						ariaLabel="auto update mode"
						value={autoUpdate}
						options={AUTO_UPDATE_OPTIONS}
						onChange={setAutoUpdate}
					/>
					<p className="mt-2 font-mono text-[11px] text-text-faint">
						never · skip checks · notify · banner only · apply · auto-update
						on detect
					</p>
				</Card>
			)}

			<div className="flex justify-end gap-2">
				<Button variant="primary" disabled={!dirty || busy} onClick={save}>
					save
				</Button>
			</div>

			{detail.status === "stopped" && (
				<Card header="danger zone">
					<div className="flex items-center justify-between gap-4">
						<p className="font-mono text-[12px] text-text-muted">
							deletes the StatefulSet, PVC, Service, and RCON Secret. the world
							data is lost.
						</p>
						<Button
							variant="danger"
							onClick={() => {
								setConfirmOpen(true);
							}}
						>
							delete server
						</Button>
					</div>
				</Card>
			)}

			<ConfirmDeleteDialog
				open={confirmOpen}
				onClose={() => {
					setConfirmOpen(false);
				}}
				targetName={detail.name}
				description="this permanently removes the StatefulSet, PVC, Service, and RCON Secret. the server's world data is lost."
				onConfirm={async () => {
					await deleteServer(detail.id);
					toast.push(`${detail.name} deleted`, "success");
					router.push("/");
				}}
			/>
		</div>
	);
}
