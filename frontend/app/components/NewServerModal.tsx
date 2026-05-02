"use client";

import {
	useEffect,
	useState,
	type FormEvent,
	type ReactElement,
	type ReactNode,
} from "react";

import {
	ApiError,
	createServer,
	fetchCapabilities,
	type ClusterCapabilities,
	type ExposureMode,
} from "../lib/api";
import { Button } from "./Button";
import { Modal } from "./Modal";

interface NewServerModalProps {
	open: boolean;
	onClose: () => void;
	onCreated: (id: string) => void;
}

const MC_VERSIONS = [
	"1.21.4",
	"1.21.3",
	"1.21.1",
	"1.21.0",
	"1.20.6",
	"1.20.4",
] as const;

const MEMORY_STEPS_MI = [1024, 2048, 4096, 6144, 8192, 12_288, 16_384] as const;

const NAME_REGEX = /^[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

interface FormState {
	name: string;
	mcVersion: string;
	memoryMi: number;
	exposureMode: ExposureMode | "";
	storageClass: string; // "" === cluster default
}

const INITIAL_STATE: FormState = {
	name: "",
	mcVersion: "1.21.4",
	memoryMi: 4096,
	exposureMode: "",
	storageClass: "",
};

type LoadState =
	| { kind: "loading" }
	| { kind: "ready"; capabilities: ClusterCapabilities }
	| { kind: "error"; message: string };

export function NewServerModal({
	open,
	onClose,
	onCreated,
}: NewServerModalProps): ReactElement {
	const [load, setLoad] = useState<LoadState>({ kind: "loading" });
	const [form, setForm] = useState<FormState>(INITIAL_STATE);
	const [submitError, setSubmitError] = useState<string | null>(null);
	const [submitting, setSubmitting] = useState(false);

	useEffect(() => {
		const ctrl = new AbortController();
		// Async fetch: setState fires only after fetchCapabilities resolves,
		// so the lint rule about synchronous setState does not apply.
		fetchCapabilities(ctrl.signal)
			.then((capabilities) => {
				setLoad({ kind: "ready", capabilities });
				const preferred: ExposureMode = capabilities.loadbalancer
					? "loadbalancer"
					: capabilities.nodeport
						? "nodeport"
						: "clusterip";
				setForm((prev) => ({ ...prev, exposureMode: preferred }));
			})
			.catch((err: unknown) => {
				if (err instanceof DOMException && err.name === "AbortError") return;
				const message = err instanceof Error ? err.message : "unknown error";
				setLoad({ kind: "error", message });
			});
		return () => {
			ctrl.abort();
		};
	}, []);

	const onSubmit = (event: FormEvent<HTMLFormElement>): void => {
		event.preventDefault();
		if (load.kind !== "ready" || form.exposureMode === "") return;
		setSubmitError(null);
		setSubmitting(true);
		const request = {
			name: form.name,
			mc_version: form.mcVersion,
			memory_mi: form.memoryMi,
			exposure_mode: form.exposureMode,
			...(form.storageClass !== "" && { storage_class: form.storageClass }),
		};
		createServer(request)
			.then((res) => {
				onCreated(res.id);
			})
			.catch((err: unknown) => {
				if (err instanceof ApiError) {
					setSubmitError(`${err.code}: ${err.message}`);
				} else {
					setSubmitError(err instanceof Error ? err.message : "unknown error");
				}
			})
			.finally(() => {
				setSubmitting(false);
			});
	};

	const nameValid = form.name.length === 0 || NAME_REGEX.test(form.name);
	const formValid = NAME_REGEX.test(form.name) && form.exposureMode !== "";

	return (
		<Modal open={open} onClose={onClose} title="New server">
			{load.kind === "loading" && (
				<p className="text-sm text-slate-400">Loading cluster capabilities…</p>
			)}
			{load.kind === "error" && (
				<p className="text-sm text-red-400">
					Failed to load cluster capabilities: {load.message}
				</p>
			)}
			{load.kind === "ready" && (
				<form onSubmit={onSubmit} className="flex flex-col gap-4">
					<Field label="Name">
						<input
							type="text"
							value={form.name}
							onChange={(e) => {
								setForm({ ...form, name: e.target.value });
							}}
							placeholder="survival"
							autoFocus
							required
							className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 font-mono text-sm focus:border-green-500 focus:outline-none"
						/>
						{!nameValid && (
							<p className="mt-1 text-xs text-red-400">
								Lowercase letters, digits, and dashes. Must start with a letter
								and end with a letter or digit. 1–63 chars.
							</p>
						)}
					</Field>

					<Field label="Minecraft version">
						<select
							value={form.mcVersion}
							onChange={(e) => {
								setForm({ ...form, mcVersion: e.target.value });
							}}
							className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 font-mono text-sm focus:border-green-500 focus:outline-none"
						>
							{MC_VERSIONS.map((v) => (
								<option key={v} value={v}>
									{v}
								</option>
							))}
						</select>
					</Field>

					<Field label={`Memory: ${(form.memoryMi / 1024).toString()} GiB`}>
						<input
							type="range"
							min={MEMORY_STEPS_MI[0]}
							max={MEMORY_STEPS_MI[MEMORY_STEPS_MI.length - 1]}
							step={1024}
							value={form.memoryMi}
							onChange={(e) => {
								setForm({ ...form, memoryMi: Number(e.target.value) });
							}}
							className="w-full accent-green-500"
						/>
					</Field>

					<Field label="Exposure mode">
						<select
							value={form.exposureMode}
							onChange={(e) => {
								setForm({
									...form,
									exposureMode: e.target.value as ExposureMode,
								});
							}}
							className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 font-mono text-sm focus:border-green-500 focus:outline-none"
						>
							{load.capabilities.loadbalancer && (
								<option value="loadbalancer">LoadBalancer</option>
							)}
							{load.capabilities.nodeport && (
								<option value="nodeport">NodePort</option>
							)}
							{load.capabilities.clusterip && (
								<option value="clusterip">ClusterIP (cluster DNS only)</option>
							)}
						</select>
					</Field>

					<Field label="Storage class">
						<select
							value={form.storageClass}
							onChange={(e) => {
								setForm({ ...form, storageClass: e.target.value });
							}}
							className="w-full rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 font-mono text-sm focus:border-green-500 focus:outline-none"
						>
							<option value="">(cluster default)</option>
							{load.capabilities.available_storage_classes.map((sc) => (
								<option key={sc} value={sc}>
									{sc}
									{load.capabilities.default_storage_class === sc
										? " (default)"
										: ""}
								</option>
							))}
						</select>
					</Field>

					{submitError !== null && (
						<p className="text-sm text-red-400">{submitError}</p>
					)}

					<div className="mt-2 flex justify-end gap-2">
						<Button variant="secondary" onClick={onClose} disabled={submitting}>
							cancel
						</Button>
						<Button
							variant="primary"
							type="submit"
							disabled={submitting || !formValid}
						>
							{submitting ? "creating…" : "create"}
						</Button>
					</div>
				</form>
			)}
		</Modal>
	);
}

interface FieldProps {
	label: string;
	children: ReactNode;
}

function Field({ label, children }: FieldProps): ReactElement {
	return (
		<label className="flex flex-col gap-1.5 text-sm">
			<span className="text-xs uppercase tracking-wide text-slate-400">
				{label}
			</span>
			{children}
		</label>
	);
}
