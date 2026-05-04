"use client";

import { useRouter } from "next/navigation";
import { useState, type ReactElement } from "react";

import {
	ApiError,
	deleteServer,
	updateServerSettings,
	type AutoUpdateMode,
} from "../../lib/api";
import { useMcVersions } from "../../lib/use-mc-versions";
import { useServerDetailCtx } from "../../lib/server-detail-context";

import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { ConfirmDeleteDialog } from "../../components/ConfirmDeleteDialog";
import { RangeSlider } from "../../components/RangeSlider";
import { SegmentedControl } from "../../components/SegmentedControl";
import { useToast } from "../../components/Toast";

const AUTO_UPDATE_OPTIONS: ReadonlyArray<{
	value: AutoUpdateMode;
	label: string;
}> = [
	{ value: "never", label: "never" },
	{ value: "notify", label: "notify" },
	{ value: "apply", label: "apply" },
];

function readAutoUpdate(
	detail: ReturnType<typeof useServerDetailCtx>,
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
	const detail = useServerDetailCtx();
	const router = useRouter();
	const toast = useToast();
	const versions = useMcVersions();

	const [memory, setMemory] = useState(detail.memory_mi);
	const [cpu, setCpu] = useState(detail.cpu_millicores);
	const [autoUpdate, setAutoUpdate] = useState<AutoUpdateMode>(
		readAutoUpdate(detail),
	);
	const [busy, setBusy] = useState(false);
	const [confirmOpen, setConfirmOpen] = useState(false);

	const isModpack = detail.source_kind !== "vanilla";

	const memoryDirty = memory !== detail.memory_mi;
	const cpuDirty = cpu !== detail.cpu_millicores;
	const autoUpdateDirty = isModpack && autoUpdate !== readAutoUpdate(detail);
	const dirty = memoryDirty || cpuDirty || autoUpdateDirty;

	const save = (): void => {
		setBusy(true);
		const patch = {
			...(memoryDirty ? { memory_mi: memory } : {}),
			...(cpuDirty ? { cpu_millicores: cpu } : {}),
			...(autoUpdateDirty ? { auto_update_mode: autoUpdate } : {}),
		};
		updateServerSettings(detail.id, patch)
			.then(() => {
				toast.push("settings saved · applies on next start", "success");
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
						max={16384}
						step={1024}
						unit="MiB"
					/>
					<RangeSlider
						label="cpu"
						value={cpu}
						onChange={setCpu}
						min={250}
						max={16000}
						step={250}
						unit="m"
					/>
				</div>
			</Card>

			<Card header="minecraft version · informational">
				<p className="font-mono text-[12px] text-text-muted">
					currently · {detail.mc_version}
				</p>
				{versions !== undefined && versions.versions.length > 0 && (
					<p className="mt-2 font-mono text-[11px] text-text-faint">
						upstream releases · {versions.versions.slice(0, 8).join(", ")}
						{versions.versions.length > 8 && " …"}
					</p>
				)}
				<p className="mt-2 font-mono text-[11px] text-text-faint">
					version changes ship with sub-project B (modpack runtime registry).
				</p>
			</Card>

			{isModpack && (
				<Card header="modpack auto-update">
					<SegmentedControl
						ariaLabel="auto update mode"
						value={autoUpdate}
						options={AUTO_UPDATE_OPTIONS}
						onChange={setAutoUpdate}
					/>
					<p className="mt-2 font-mono text-[11px] text-text-faint">
						never · pin · notify · banner only · apply · auto-update on detect
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
