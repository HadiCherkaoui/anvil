"use client";

import {
	createContext,
	useContext,
	type ReactElement,
	type ReactNode,
} from "react";

import type { CfChannel, ExposureMode, SourceKind } from "../lib/api";

export type CreateType = "vanilla" | "modpack";

export interface CreateDraft {
	name: string;
	type: CreateType;
	mc_version: string | null;
	cpu_millicores: number;
	memory_mi: number;
	storage_size_gi: number;
	storage_class: string | null;
	exposure_mode: ExposureMode;
	server_type: SourceKind;
	curseforge: { project_id: number; channel: CfChannel } | null;
}

export const CreateFormContext = createContext<CreateDraft | null>(null);

function useDraft(): CreateDraft {
	const v = useContext(CreateFormContext);
	if (!v) {
		throw new Error("BuildSlip must be used inside CreateFormContext");
	}
	return v;
}

function dash(v: string | number | null | undefined): string {
	if (v === null || v === undefined || v === "") return "—";
	return String(v);
}

type Status = "draft" | "valid" | "submitting";

const STATUS_TEXT: Record<Status, string> = {
	draft: "draft",
	valid: "ready",
	submitting: "forging…",
};

const STATUS_TONE: Record<Status, string> = {
	draft: "text-text-muted",
	valid: "text-state-running",
	submitting: "text-accent",
};

export function BuildSlip({ status }: { status: Status }): ReactElement {
	const d = useDraft();
	return (
		<aside className="sticky top-6 w-80 self-start rounded-md border border-border bg-surface p-5">
			<header className="mb-4 flex items-center justify-between">
				<span className="font-mono text-[10px] uppercase tracking-[0.12em] text-text-faint">
					build slip
				</span>
				<span
					className={`font-mono text-[11px] uppercase tracking-wider ${STATUS_TONE[status]}`}
				>
					{STATUS_TEXT[status]}
				</span>
			</header>
			<dl className="grid grid-cols-1 gap-y-3 font-mono text-[12px]">
				<Section label="01 identity">
					<Field label="name" value={d.name} />
					<Field label="type" value={d.type} />
				</Section>
				<Section label="02 source">
					<Field label="mc version" value={d.mc_version} />
					{d.type === "modpack" && d.curseforge !== null && (
						<>
							<Field label="cf project" value={d.curseforge.project_id} />
							<Field label="channel" value={d.curseforge.channel} />
						</>
					)}
				</Section>
				<Section label="03 resources">
					<Field
						label="cpu"
						value={`${(d.cpu_millicores / 1000).toFixed(2)} cores`}
					/>
					<Field label="memory" value={`${d.memory_mi.toString()} MiB`} />
				</Section>
				<Section label="04 storage">
					<Field label="size" value={`${d.storage_size_gi.toString()} GiB`} />
					<Field label="class" value={d.storage_class} />
				</Section>
				<Section label="05 network">
					<Field label="exposure" value={d.exposure_mode} />
				</Section>
			</dl>
		</aside>
	);
}

function Section({
	label,
	children,
}: {
	label: string;
	children: ReactNode;
}): ReactElement {
	return (
		<div className="border-t border-border-soft pt-2 first:border-t-0 first:pt-0">
			<div className="mb-1 font-mono text-[10px] uppercase tracking-[0.12em] text-text-faint">
				{label}
			</div>
			{children}
		</div>
	);
}

function Field({
	label,
	value,
}: {
	label: string;
	value: string | number | null | undefined;
}): ReactElement {
	return (
		<div className="flex justify-between">
			<span className="text-text-muted">{label}</span>
			<span className="text-text-body">{dash(value)}</span>
		</div>
	);
}
