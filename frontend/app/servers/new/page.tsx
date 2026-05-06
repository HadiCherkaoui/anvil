"use client";

import { useRouter } from "next/navigation";
import {
	useEffect,
	useMemo,
	useState,
	type ChangeEvent,
	type ReactElement,
	type ReactNode,
} from "react";

import {
	ApiError,
	createServer,
	DEFAULT_PROPERTIES,
	fetchCapabilities,
	type CfChannel,
	type ClusterCapabilities,
	type CreateServerRequest,
	type Difficulty,
	type ExposureMode,
	type Gamemode,
	type ModEntry,
	type Runtime,
} from "../../lib/api";
import { useLoaderVersions } from "../../lib/use-loader-versions";
import { useMcVersions } from "../../lib/use-mc-versions";

import {
	BuildSlip,
	CreateFormContext,
	type CreateDraft,
	type CreateType,
} from "../../components/BuildSlip";
import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { CatalogSheet, type CatalogPick } from "../../components/CatalogSheet";
import { RangeSlider } from "../../components/RangeSlider";
import { SegmentedControl } from "../../components/SegmentedControl";
import { useToast } from "../../components/Toast";

const TYPE_OPTIONS: ReadonlyArray<{ value: CreateType; label: string }> = [
	{ value: "vanilla", label: "vanilla" },
	{ value: "paper", label: "paper" },
	{ value: "modpack", label: "modpack" },
	{ value: "modded", label: "modded" },
];

const CHANNEL_OPTIONS: ReadonlyArray<{ value: CfChannel; label: string }> = [
	{ value: "release", label: "release" },
	{ value: "beta", label: "beta" },
	{ value: "alpha", label: "alpha" },
];

const RUNTIME_OPTIONS: ReadonlyArray<{ value: Runtime; label: string }> = [
	{ value: "fabric", label: "fabric" },
	{ value: "forge", label: "forge" },
	{ value: "neoforge", label: "neoforge" },
];

const DIFFICULTY_OPTIONS: ReadonlyArray<{ value: Difficulty; label: string }> = [
	{ value: "peaceful", label: "peaceful" },
	{ value: "easy", label: "easy" },
	{ value: "normal", label: "normal" },
	{ value: "hard", label: "hard" },
];

const GAMEMODE_OPTIONS: ReadonlyArray<{ value: Gamemode; label: string }> = [
	{ value: "survival", label: "survival" },
	{ value: "creative", label: "creative" },
	{ value: "adventure", label: "adventure" },
	{ value: "spectator", label: "spectator" },
];

const HARDCORE_OPTIONS: ReadonlyArray<{ value: "off" | "on"; label: string }> = [
	{ value: "off", label: "off" },
	{ value: "on", label: "on" },
];

const INITIAL: CreateDraft = {
	name: "",
	type: "vanilla",
	mc_version: null,
	memory_mi: 4096,
	storage_size_gi: 20,
	storage_class: null,
	// loadbalancer is the right default when the cluster can give the
	// server a real public IP. The capabilities-load effect downgrades
	// this to `clusterip` if the cluster lacks an LB provider.
	exposure_mode: "loadbalancer",
	curseforge: null,
	modrinth: null,
	runtime: null,
	initial_mods: [],
	initial_plugins: [],
	loader_version: null,
	properties: DEFAULT_PROPERTIES,
};

function buildExposureOptions(
	caps: ClusterCapabilities | null,
): ReadonlyArray<{ value: ExposureMode; label: string }> {
	const opts: Array<{ value: ExposureMode; label: string }> = [
		{ value: "clusterip", label: "clusterip" },
	];
	if (caps?.nodeport ?? true)
		opts.push({ value: "nodeport", label: "nodeport" });
	if (caps?.loadbalancer === true) {
		opts.push({ value: "loadbalancer", label: "loadbalancer" });
	}
	return opts;
}

export default function NewServerPage(): ReactElement {
	const router = useRouter();
	const toast = useToast();
	const versions = useMcVersions();
	const [draft, setDraft] = useState<CreateDraft>(INITIAL);
	const [submitting, setSubmitting] = useState(false);
	const [caps, setCaps] = useState<ClusterCapabilities | null>(null);
	const [browseOpen, setBrowseOpen] = useState(false);

	useEffect(() => {
		const ctrl = new AbortController();
		fetchCapabilities(ctrl.signal)
			.then((c) => {
				setCaps(c);
				// Clamp the (loadbalancer-by-default) exposure_mode down to
				// something the cluster actually supports. Only touches the
				// field if the current selection is unavailable, so a user
				// who already picked something keeps their choice.
				setDraft((d) => {
					if (d.exposure_mode === "loadbalancer" && !c.loadbalancer) {
						return {
							...d,
							exposure_mode: c.nodeport ? "nodeport" : "clusterip",
						};
					}
					if (d.exposure_mode === "nodeport" && !c.nodeport) {
						return { ...d, exposure_mode: "clusterip" };
					}
					return d;
				});
			})
			.catch(() => {
				// best-effort — exposure dropdown falls back to clusterip-only
			});
		return () => {
			ctrl.abort();
		};
	}, []);

	const set = <K extends keyof CreateDraft>(k: K, v: CreateDraft[K]): void => {
		setDraft((d) => ({ ...d, [k]: v }));
	};

	const exposureOptions = useMemo(() => buildExposureOptions(caps), [caps]);

	const missing: string[] = [];
	if (draft.name === "") missing.push("name");
	if (draft.type !== "modpack") {
		if (draft.mc_version === null || draft.mc_version === "")
			missing.push("mc version");
	}
	if (draft.type === "modpack") {
		if (draft.curseforge === null && draft.modrinth === null)
			missing.push("modpack");
	}
	if (draft.type === "modded" && draft.runtime === null)
		missing.push("runtime");
	const valid = missing.length === 0;
	const status = submitting ? "submitting" : valid ? "valid" : "draft";

	const onCatalogPick = (pick: CatalogPick): void => {
		if (draft.type === "modpack") {
			if (pick.hit.provider === "modrinth") {
				set("modrinth", {
					project_id: pick.hit.project_id,
					channel: "release",
				});
				set("curseforge", null);
				set("mc_version", pick.version.version_name);
			} else {
				const idNum = Number.parseInt(pick.hit.project_id, 10);
				if (!Number.isNaN(idNum)) {
					set("curseforge", { project_id: idNum, channel: "release" });
					set("modrinth", null);
					set("mc_version", pick.version.version_name);
				}
			}
		} else if (draft.type === "modded") {
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
			set("initial_mods", [...draft.initial_mods, entry]);
		} else if (draft.type === "paper") {
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
			set("initial_plugins", [...draft.initial_plugins, entry]);
		}
	};

	const switchRuntimeWithGuard = (next: Runtime): void => {
		if (
			draft.initial_mods.length > 0 &&
			!window.confirm(
				`switching runtime clears ${draft.initial_mods.length.toString()} picked mods. continue?`,
			)
		) {
			return;
		}
		set("runtime", next);
		set("initial_mods", []);
		set("loader_version", null);
	};

	const switchMcWithGuard = (next: string | null): void => {
		if (
			draft.type === "modded" &&
			draft.initial_mods.length > 0 &&
			next !== draft.mc_version &&
			!window.confirm(
				`switching mc version clears ${draft.initial_mods.length.toString()} picked mods. continue?`,
			)
		) {
			return;
		}
		if (
			draft.type === "paper" &&
			draft.initial_plugins.length > 0 &&
			next !== draft.mc_version &&
			!window.confirm(
				`switching mc version clears ${draft.initial_plugins.length.toString()} picked plugins. continue?`,
			)
		) {
			return;
		}
		set("mc_version", next);
		if (draft.type === "modded") {
			set("initial_mods", []);
			set("loader_version", null);
		}
		if (draft.type === "paper") {
			set("initial_plugins", []);
		}
	};

	const submit = (): void => {
		if (!valid) return;
		setSubmitting(true);
		const isPaper = draft.type === "paper";
		const isModpack =
			draft.type === "modpack" &&
			(draft.curseforge !== null || draft.modrinth !== null);
		const isModded = draft.type === "modded" && draft.runtime !== null;

		const sourceKind: CreateServerRequest["source_kind"] = isPaper
			? "paper"
			: isModpack
				? draft.modrinth !== null
					? "modrinth"
					: "curseforge"
				: isModded
					? "modded"
					: "vanilla";

		const request: CreateServerRequest = {
			name: draft.name,
			memory_mi: draft.memory_mi,
			exposure_mode: draft.exposure_mode,
			storage_size_gi: draft.storage_size_gi,
			...(draft.storage_class !== null && draft.storage_class !== ""
				? { storage_class: draft.storage_class }
				: {}),
			...(draft.mc_version !== null ? { mc_version: draft.mc_version } : {}),
			source_kind: sourceKind,
			...(isModpack && draft.curseforge !== null && draft.modrinth === null
				? {
						curseforge: {
							project_id: draft.curseforge.project_id,
							channel: draft.curseforge.channel,
						},
					}
				: {}),
			...(isModpack && draft.modrinth !== null
				? {
						modrinth: {
							project_id: draft.modrinth.project_id,
							channel: draft.modrinth.channel,
						},
					}
				: {}),
			...(isModded && draft.runtime !== null
				? {
						modded: {
							runtime: draft.runtime,
							initial_mods: draft.initial_mods,
							...(effectiveLoaderVersion !== null
								? { loader_version: effectiveLoaderVersion }
								: {}),
						},
					}
				: {}),
			...(isPaper && draft.initial_plugins.length > 0
				? { paper: { initial_plugins: draft.initial_plugins } }
				: {}),
			properties: draft.properties,
		};
		createServer(request)
			.then((created) => {
				toast.push(`${created.name} forged`, "success");
				router.push(`/servers?name=${encodeURIComponent(created.name)}`);
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`create failed · ${msg}`, "error");
				setSubmitting(false);
			});
	};

	const loaderRuntime: "forge" | "neoforge" | null =
		draft.type === "modded" &&
		(draft.runtime === "forge" || draft.runtime === "neoforge")
			? draft.runtime
			: null;
	const loaderVs = useLoaderVersions(loaderRuntime);
	const moddedMcOptions: readonly string[] = useMemo(
		() =>
			loaderRuntime !== null
				? (loaderVs?.mc_versions ?? [])
				: (versions?.versions ?? []),
		[loaderRuntime, loaderVs, versions],
	);
	const loaderChoicesForMc: readonly string[] = useMemo(
		() =>
			loaderRuntime !== null && draft.mc_version !== null
				? (loaderVs?.by_mc[draft.mc_version] ?? [])
				: [],
		[loaderRuntime, draft.mc_version, loaderVs],
	);
	// Default to the newest loader for the current MC when the user hasn't
	// picked one explicitly. Derived rather than stored to avoid a setState-
	// in-effect dance — the request submit reads this directly.
	const effectiveLoaderVersion: string | null =
		loaderRuntime !== null && draft.mc_version !== null
			? (draft.loader_version ?? loaderChoicesForMc[0] ?? null)
			: null;

	const browseMode: "modpack" | "mod" | "plugin" =
		draft.type === "modded"
			? "mod"
			: draft.type === "paper"
				? "plugin"
				: "modpack";
	const browseLoader: Runtime | "paper" | undefined =
		draft.type === "modded" && draft.runtime !== null
			? draft.runtime
			: draft.type === "paper"
				? "paper"
				: undefined;
	const browseMc: string | undefined =
		(draft.type === "modded" || draft.type === "paper") &&
		draft.mc_version !== null
			? draft.mc_version
			: undefined;

	return (
		<CreateFormContext.Provider value={draft}>
			<div className="grid grid-cols-1 gap-8 px-5 py-6 lg:grid-cols-[320px_1fr]">
				<BuildSlip status={status} />
				<div className="flex max-w-2xl flex-col gap-4">
					<Section number="01" title="identity">
						<Card>
							<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
								name
							</label>
							<input
								value={draft.name}
								onChange={(e: ChangeEvent<HTMLInputElement>) => {
									set("name", e.target.value);
								}}
								placeholder="e.g. atm-11-friends"
								className="w-full rounded-md border border-border bg-bg px-3 py-2 font-mono text-[13px] text-text-body placeholder:text-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
								autoComplete="off"
								spellCheck={false}
							/>
							<p className="mt-1 font-mono text-[11px] text-text-faint">
								lowercase, dashes, 1–63 chars, must start with a letter.
							</p>
						</Card>
					</Section>

					<Section number="02" title="type">
						<Card>
							<SegmentedControl
								ariaLabel="server type"
								value={draft.type}
								onChange={(v) => {
									set("type", v);
									if (v !== "modpack") {
										set("curseforge", null);
										set("modrinth", null);
									}
									if (v !== "modded") {
										set("runtime", null);
										set("initial_mods", []);
										set("loader_version", null);
									} else {
										set("runtime", "fabric");
										set("initial_mods", []);
										set("loader_version", null);
									}
									if (v !== "paper") {
										set("initial_plugins", []);
									}
								}}
								options={TYPE_OPTIONS}
							/>
						</Card>
					</Section>

					<Section number="03" title="source">
						<Card>
							{draft.type === "vanilla" || draft.type === "paper" ? (
								<div className="flex flex-col gap-3">
									<McVersionPicker
										value={draft.mc_version}
										onChange={(v) => {
											switchMcWithGuard(v);
										}}
										versions={versions?.versions ?? []}
										showFallbackWarning={versions?.source === "fallback"}
									/>
									{draft.type === "paper" && (
										<>
											<div className="flex items-center gap-2">
												<Button
													onClick={() => {
														if (draft.mc_version !== null) setBrowseOpen(true);
													}}
													disabled={draft.mc_version === null}
												>
													+ pre-pick plugins
												</Button>
												<span className="font-mono text-[11px] text-text-faint">
													{draft.initial_plugins.length} picked
												</span>
											</div>
											{draft.initial_plugins.length > 0 && (
												<ul className="mt-2 flex flex-col gap-1">
													{draft.initial_plugins.map((p, i) => (
														<li
															key={`${p.provider}:${p.version_id}`}
															className="flex items-center gap-2 rounded-md border border-border bg-surface px-2 py-1"
														>
															<span className="font-mono text-[12px] text-text-body">
																{p.project_name}
															</span>
															<span className="font-mono text-[11px] text-text-faint">
																{p.version_name}
															</span>
															<button
																type="button"
																onClick={() => {
																	set(
																		"initial_plugins",
																		draft.initial_plugins.filter(
																			(_, j) => j !== i,
																		),
																	);
																}}
																aria-label={`remove ${p.project_name}`}
																className="ml-auto rounded p-1 text-text-faint hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
															>
																×
															</button>
														</li>
													))}
												</ul>
											)}
										</>
									)}
								</div>
							) : draft.type === "modded" ? (
								<div className="flex flex-col gap-3">
									{draft.runtime !== null && (
										<div>
											<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
												runtime
											</label>
											<SegmentedControl
												ariaLabel="modded runtime"
												value={draft.runtime}
												onChange={(v) => {
													switchRuntimeWithGuard(v);
												}}
												options={RUNTIME_OPTIONS}
											/>
										</div>
									)}
									{loaderRuntime !== null && loaderVs === null && (
										<p className="font-mono text-[11px] text-text-faint">
											loading {loaderRuntime} versions…
										</p>
									)}
									<McVersionPicker
										value={draft.mc_version}
										onChange={(v) => {
											switchMcWithGuard(v);
										}}
										versions={moddedMcOptions}
										showFallbackWarning={
											loaderRuntime === null &&
											versions?.source === "fallback"
										}
									/>
									{loaderRuntime !== null && draft.mc_version !== null && (
										<div>
											<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
												{loaderRuntime} version
											</label>
											<select
												value={effectiveLoaderVersion ?? ""}
												onChange={(e) => {
													set(
														"loader_version",
														e.target.value === "" ? null : e.target.value,
													);
												}}
												className="rounded-md border border-border bg-bg px-2 py-1.5 font-mono text-[12px] text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
											>
												<option value="">— select —</option>
												{loaderChoicesForMc.map((v) => (
													<option key={v} value={v}>
														{v}
													</option>
												))}
											</select>
										</div>
									)}
									<div className="flex items-center gap-2">
										<Button
											onClick={() => {
												if (draft.runtime !== null && draft.mc_version !== null)
													setBrowseOpen(true);
											}}
											disabled={
												draft.runtime === null || draft.mc_version === null
											}
										>
											+ pre-pick mods
										</Button>
										<span className="font-mono text-[11px] text-text-faint">
											{draft.initial_mods.length} picked
										</span>
									</div>
									{draft.initial_mods.length > 0 && (
										<ul className="mt-2 flex flex-col gap-1">
											{draft.initial_mods.map((m, i) => (
												<li
													key={`${m.provider}:${m.version_id}`}
													className="flex items-center gap-2 rounded-md border border-border bg-surface px-2 py-1"
												>
													<span className="font-mono text-[12px] text-text-body">
														{m.project_name}
													</span>
													<span className="font-mono text-[11px] text-text-faint">
														{m.version_name}
													</span>
													<button
														type="button"
														onClick={() => {
															set(
																"initial_mods",
																draft.initial_mods.filter((_, j) => j !== i),
															);
														}}
														aria-label={`remove ${m.project_name}`}
														className="ml-auto rounded p-1 text-text-faint hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
													>
														×
													</button>
												</li>
											))}
										</ul>
									)}
								</div>
							) : (
								<div className="flex flex-col gap-3">
									<Button
										onClick={() => {
											setBrowseOpen(true);
										}}
									>
										browse
									</Button>
									{draft.curseforge !== null && (
										<>
											<p className="font-mono text-[12px] text-text-body">
												curseforge project ·{" "}
												{draft.curseforge.project_id.toString()}
											</p>
											<div>
												<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
													channel
												</label>
												<SegmentedControl
													ariaLabel="release channel"
													value={draft.curseforge.channel}
													onChange={(v) => {
														if (draft.curseforge !== null) {
															set("curseforge", {
																project_id: draft.curseforge.project_id,
																channel: v,
															});
														}
													}}
													options={CHANNEL_OPTIONS}
												/>
											</div>
											<div>
												<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
													label this build as
												</label>
												<input
													value={draft.mc_version ?? ""}
													onChange={(e) => {
														set(
															"mc_version",
															e.target.value === "" ? null : e.target.value,
														);
													}}
													placeholder="e.g. atm-11-4.4"
													className="w-full rounded-md border border-border bg-bg px-3 py-2 font-mono text-[12px] text-text-body placeholder:text-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
													spellCheck={false}
												/>
											</div>
										</>
									)}
									{draft.modrinth !== null && (
										<>
											<p className="font-mono text-[12px] text-text-body">
												modrinth project · {draft.modrinth.project_id}
											</p>
											<div>
												<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
													channel
												</label>
												<SegmentedControl
													ariaLabel="release channel"
													value={draft.modrinth.channel}
													onChange={(v) => {
														if (draft.modrinth !== null) {
															set("modrinth", {
																project_id: draft.modrinth.project_id,
																channel: v,
															});
														}
													}}
													options={CHANNEL_OPTIONS}
												/>
											</div>
										</>
									)}
								</div>
							)}
						</Card>
					</Section>

					<Section number="04" title="resources">
						<Card>
							<div className="flex flex-col gap-4">
								<RangeSlider
									label="memory"
									value={draft.memory_mi}
									onChange={(v) => {
										set("memory_mi", v);
									}}
									min={1024}
									max={65536}
									step={1024}
									unit="MiB"
								/>
							</div>
						</Card>
					</Section>

					<Section number="05" title="world">
						<Card>
							<div className="flex flex-col gap-4">
								<div>
									<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
										difficulty
									</label>
									<SegmentedControl
										ariaLabel="difficulty"
										value={draft.properties.difficulty}
										onChange={(v) => {
											set("properties", {
												...draft.properties,
												difficulty: v,
											});
										}}
										options={DIFFICULTY_OPTIONS}
									/>
								</div>
								<div>
									<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
										gamemode
									</label>
									<SegmentedControl
										ariaLabel="gamemode"
										value={draft.properties.gamemode}
										onChange={(v) => {
											set("properties", {
												...draft.properties,
												gamemode: v,
											});
										}}
										options={GAMEMODE_OPTIONS}
									/>
								</div>
								<div>
									<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
										hardcore
									</label>
									<SegmentedControl
										ariaLabel="hardcore"
										value={draft.properties.hardcore ? "on" : "off"}
										onChange={(v) => {
											set("properties", {
												...draft.properties,
												hardcore: v === "on",
											});
										}}
										options={HARDCORE_OPTIONS}
									/>
									<p className="mt-1 font-mono text-[11px] text-text-faint">
										hardcore bans players on death — only meaningful from a
										fresh world
									</p>
								</div>
							</div>
						</Card>
					</Section>

					<Section number="06" title="storage">
						<Card>
							<div className="flex flex-col gap-4">
								<RangeSlider
									label="size"
									value={draft.storage_size_gi}
									onChange={(v) => {
										set("storage_size_gi", v);
									}}
									min={10}
									max={500}
									step={10}
									unit="GiB"
								/>
								{caps !== null && caps.available_storage_classes.length > 1 && (
									<div>
										<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
											storage class
										</label>
										<select
											value={draft.storage_class ?? ""}
											onChange={(e) => {
												set(
													"storage_class",
													e.target.value === "" ? null : e.target.value,
												);
											}}
											className="rounded-md border border-border bg-bg px-2 py-1.5 font-mono text-[12px] text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
										>
											<option value="">
												— default ({caps.default_storage_class ?? "?"}) —
											</option>
											{caps.available_storage_classes.map((c) => (
												<option key={c} value={c}>
													{c}
												</option>
											))}
										</select>
									</div>
								)}
							</div>
						</Card>
					</Section>

					<Section number="07" title="network">
						<Card>
							<SegmentedControl
								ariaLabel="exposure mode"
								value={draft.exposure_mode}
								onChange={(v) => {
									set("exposure_mode", v);
								}}
								options={exposureOptions}
							/>
						</Card>
					</Section>

					<footer className="sticky bottom-0 -mx-5 mt-4 flex items-center justify-between border-t border-border bg-bg px-5 py-3">
						<span className="font-mono text-[12px]">
							{valid ? (
								<span className="text-state-running">
									● all sections valid · ready to forge
								</span>
							) : (
								<span className="text-state-error">
									× missing: {missing.join(", ")}
								</span>
							)}
						</span>
						<div className="flex gap-2">
							<Button
								onClick={() => {
									router.push("/");
								}}
							>
								cancel
							</Button>
							<Button
								variant="primary"
								disabled={!valid || submitting}
								onClick={submit}
							>
								create server
							</Button>
						</div>
					</footer>
				</div>
			</div>

			<CatalogSheet
				isOpen={browseOpen}
				onClose={() => {
					setBrowseOpen(false);
				}}
				mode={browseMode}
				{...(browseLoader !== undefined ? { loader: browseLoader } : {})}
				{...(browseMc !== undefined ? { mc: browseMc } : {})}
				onPick={onCatalogPick}
			/>
		</CreateFormContext.Provider>
	);
}

function McVersionPicker({
	value,
	onChange,
	versions,
	showFallbackWarning,
}: {
	value: string | null;
	onChange: (v: string | null) => void;
	versions: readonly string[];
	showFallbackWarning: boolean;
}): ReactElement {
	return (
		<div>
			<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
				minecraft version
			</label>
			<select
				value={value ?? ""}
				onChange={(e) => {
					onChange(e.target.value === "" ? null : e.target.value);
				}}
				className="rounded-md border border-border bg-bg px-2 py-1.5 font-mono text-[12px] text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			>
				<option value="">— select —</option>
				{versions.map((v) => (
					<option key={v} value={v}>
						{v}
					</option>
				))}
			</select>
			{showFallbackWarning && (
				<p className="mt-1 font-mono text-[11px] text-state-warning">
					mojang manifest unreachable · using offline fallback list
				</p>
			)}
		</div>
	);
}

function Section({
	number,
	title,
	children,
}: {
	number: string;
	title: string;
	children: ReactNode;
}): ReactElement {
	return (
		<section>
			<header className="mb-2 flex items-baseline gap-3">
				<span className="font-mono text-[10px] uppercase tracking-[0.12em] text-text-faint">
					{number}
				</span>
				<h2 className="font-mono text-[14px] uppercase tracking-wider text-text-primary">
					{title}
				</h2>
			</header>
			{children}
		</section>
	);
}
