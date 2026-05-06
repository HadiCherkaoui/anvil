"use client";

import { useMemo, useState, type ReactElement } from "react";

import {
	ApiError,
	changeServerVersion,
	type ServerDetail,
} from "../lib/api";
import { useLoaderVersions } from "../lib/use-loader-versions";
import { useMcVersions } from "../lib/use-mc-versions";
import { useServerDetail } from "../lib/server-detail-context";

import { Button } from "./Button";
import { Sheet } from "./Sheet";
import { useToast } from "./Toast";
import { UpdateSheet } from "./UpdateSheet";

interface Props {
	isOpen: boolean;
	onClose: () => void;
	detail: ServerDetail;
}

interface ParsedSource {
	loaderRuntime: "forge" | "neoforge" | null;
	currentLoader: string | null;
}

function parseSource(detail: ServerDetail): ParsedSource {
	if (detail.source_kind !== "modded") {
		return { loaderRuntime: null, currentLoader: null };
	}
	const cfg = detail.source_config;
	if (cfg === null || typeof cfg !== "object") {
		return { loaderRuntime: null, currentLoader: null };
	}
	const runtime = (cfg as { runtime?: unknown }).runtime;
	const loader = (cfg as { loader_version?: unknown }).loader_version;
	const r =
		runtime === "forge" || runtime === "neoforge" ? runtime : null;
	const l = typeof loader === "string" && loader.length > 0 ? loader : null;
	return { loaderRuntime: r, currentLoader: l };
}

export function VersionChangeSheet({
	isOpen,
	onClose,
	detail,
}: Props): ReactElement {
	const { refresh } = useServerDetail();
	const [progressOpen, setProgressOpen] = useState(false);

	return (
		<>
			<Sheet isOpen={isOpen} onClose={onClose} title="change mc version">
				{/* Mount the form only when the sheet is open so picker state
				    re-initializes from the latest detail on each open without a
				    syncing useEffect. */}
				{isOpen && (
					<VersionChangeForm
						detail={detail}
						onCancel={onClose}
						onStarted={() => {
							onClose();
							setProgressOpen(true);
						}}
					/>
				)}
			</Sheet>
			<UpdateSheet
				serverId={detail.id}
				isOpen={progressOpen}
				onClose={() => {
					setProgressOpen(false);
					refresh();
				}}
			/>
		</>
	);
}

interface FormProps {
	detail: ServerDetail;
	onCancel: () => void;
	onStarted: () => void;
}

function VersionChangeForm({
	detail,
	onCancel,
	onStarted,
}: FormProps): ReactElement {
	const toast = useToast();
	const versions = useMcVersions();

	const { loaderRuntime, currentLoader } = useMemo(
		() => parseSource(detail),
		[detail],
	);
	const loaderVs = useLoaderVersions(loaderRuntime);

	const [pickedMc, setPickedMc] = useState<string>(detail.mc_version);
	const [pickedLoader, setPickedLoader] = useState<string | null>(currentLoader);
	const [submitting, setSubmitting] = useState(false);

	const mcOptions: ReadonlyArray<string> =
		loaderRuntime !== null
			? (loaderVs?.mc_versions ?? [])
			: (versions?.versions ?? []);

	const loaderOptions: ReadonlyArray<string> =
		loaderRuntime !== null && pickedMc !== "" && loaderVs !== null
			? (loaderVs.by_mc[pickedMc] ?? [])
			: [];

	const noChange =
		pickedMc === detail.mc_version && pickedLoader === currentLoader;
	const loaderMissing = loaderRuntime !== null && pickedLoader === null;
	const canSubmit =
		!submitting && pickedMc !== "" && !loaderMissing && !noChange;

	const onSubmit = (): void => {
		if (pickedMc === "") return;
		setSubmitting(true);
		const body: { mc_version: string; loader_version?: string } = {
			mc_version: pickedMc,
		};
		if (pickedLoader !== null) {
			body.loader_version = pickedLoader;
		}
		changeServerVersion(detail.id, body)
			.then(() => {
				onStarted();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`version change failed · ${msg}`, "error");
			})
			.finally(() => {
				setSubmitting(false);
			});
	};

	return (
		<div className="flex flex-col gap-4 p-5">
			<label className="flex flex-col gap-1">
				<span className="font-mono text-[11px] uppercase tracking-wider text-text-muted">
					mc version
				</span>
				<select
					className="rounded border border-border bg-bg px-2 py-1 font-mono text-[12px] text-text-body"
					value={pickedMc}
					onChange={(e) => {
						setPickedMc(e.target.value);
						// Reset loader when MC changes — picked loader may not be
						// available for the new MC.
						setPickedLoader(null);
					}}
				>
					<option value="">— pick mc —</option>
					{mcOptions.map((m) => (
						<option key={m} value={m}>
							{m}
						</option>
					))}
				</select>
			</label>

			{loaderRuntime !== null && (
				<label className="flex flex-col gap-1">
					<span className="font-mono text-[11px] uppercase tracking-wider text-text-muted">
						{loaderRuntime} version
					</span>
					{loaderVs === null ? (
						<p className="font-mono text-[11px] text-text-faint">
							loading {loaderRuntime} versions…
						</p>
					) : (
						<select
							className="rounded border border-border bg-bg px-2 py-1 font-mono text-[12px] text-text-body"
							value={pickedLoader ?? ""}
							onChange={(e) => {
								setPickedLoader(
									e.target.value === "" ? null : e.target.value,
								);
							}}
							disabled={pickedMc === ""}
						>
							<option value="">— pick {loaderRuntime} version —</option>
							{loaderOptions.map((v) => (
								<option key={v} value={v}>
									{v}
								</option>
							))}
						</select>
					)}
				</label>
			)}

			<p className="rounded border border-border bg-surface p-3 font-mono text-[11px] text-text-faint">
				this stops the server, snapshots data, swaps in the new version, and
				restarts. world data may not migrate cleanly across major versions. on
				failure the server auto-restores from the snapshot.
			</p>

			<div className="flex justify-end gap-2">
				<Button onClick={onCancel} variant="secondary">
					cancel
				</Button>
				<Button onClick={onSubmit} variant="primary" disabled={!canSubmit}>
					change version
				</Button>
			</div>
		</div>
	);
}
