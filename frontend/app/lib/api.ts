// Network-boundary types for the panel API. Every shape is validated
// with Zod at runtime so a backend-deploy drift is caught immediately
// rather than silently rendering garbage.

import { z } from "zod";

// --- shared ---------------------------------------------------------------

const endpointSchema = z.object({
	host: z.string(),
	port: z.number().int().nonnegative(),
});

export const serverStatusSchema = z.enum([
	"running",
	"stopped",
	"starting",
	"stopping",
	"error",
]);

export const exposureModeSchema = z.enum([
	"loadbalancer",
	"nodeport",
	"clusterip",
]);

const errorResponseSchema = z.object({
	error: z.string(),
	code: z.string(),
});

// --- list -----------------------------------------------------------------

export const sourceKindSchema = z.enum([
	"vanilla",
	"curseforge",
	"modrinth",
	"modded",
	"paper",
]);

export const serverSummarySchema = z.object({
	id: z.string().uuid(),
	name: z.string(),
	status: serverStatusSchema,
	mc_version: z.string(),
	memory_mi: z.number().int().nonnegative(),
	cpu_millicores: z.number().int().nonnegative(),
	exposure_mode: exposureModeSchema,
	endpoint: endpointSchema.nullable(),
	created_at: z.number().int(),
	// M5: modpack provenance + update awareness. Defaults keep existing
	// vanilla rows shaped the same as before for callers that ignore them.
	source_kind: sourceKindSchema.default("vanilla"),
	update_available: z.boolean().default(false),
	latest_version_name: z.string().nullable().default(null),
	update_in_progress: z.boolean().default(false),
});

export const serversResponseSchema = z.object({
	servers: z.array(serverSummarySchema),
});

// --- detail ---------------------------------------------------------------

export const serverDetailSchema = serverSummarySchema.extend({
	server_type: z.string(),
	storage_class: z.string().nullable(),
	storage_size_gi: z.number().int().nonnegative(),
	nodeport: z.number().int().nullable(),
	last_started_at: z.number().int().nullable(),
	// M5: parsed provider config (passthrough JSON value) and the latest
	// cached upstream version, when any.
	source_config: z.unknown().default(null),
	latest_version_id: z.number().int().nullable().default(null),
});

// --- capabilities ---------------------------------------------------------

export const clusterCapabilitiesSchema = z.object({
	loadbalancer: z.boolean(),
	nodeport: z.boolean(),
	clusterip: z.boolean(),
	available_storage_classes: z.array(z.string()),
	default_storage_class: z.string().nullable(),
	// M5: gates the CurseForge option in the New Server modal.
	cf_api_key_present: z.boolean().default(false),
	// B: Modrinth is API-key-free; flag exists for symmetry / future failure surfaces.
	modrinth_enabled: z.boolean().default(true),
	// M6: sum of allocatable CPU across schedulable nodes (cores).
	available_cpu_cores: z.number().nonnegative().default(0),
});

export const mcVersionsResponseSchema = z.object({
	versions: z.array(z.string()).min(1),
	source: z.enum(["mojang", "fallback"]),
});

// --- logs -----------------------------------------------------------------

export const logsResponseSchema = z.object({
	lines: z.array(z.string()),
});

// --- rcon -----------------------------------------------------------------

export const rconResponseSchema = z.object({
	output: z.string(),
});

// --- auth -----------------------------------------------------------------

export const meSchema = z.object({
	sub: z.string(),
	name: z.string(),
	email: z.string(),
	picture: z.string().nullable(),
});

const logoutResponseSchema = z.object({ logoutUrl: z.string() });

// --- create ---------------------------------------------------------------

const NAME_REGEX = /^[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

export const cfChannelSchema = z.enum(["release", "beta", "alpha"]);

export const curseforgeCreateSchema = z.object({
	project_id: z.number().int().positive(),
	channel: cfChannelSchema,
});

export const modrinthCreateSchema = z.object({
	project_id: z.string().min(1).max(40),
	channel: cfChannelSchema,
});

export const runtimeSchema = z.enum(["fabric", "forge", "neoforge"]);

export const modEntrySchema = z.object({
	provider: z.enum(["curseforge", "modrinth"]),
	project_id: z.string(),
	project_slug: z.string(),
	project_name: z.string(),
	version_id: z.string(),
	version_name: z.string(),
	filename: z.string(),
	download_url: z.string(),
	sha512: z.string().nullable().default(null),
});

export const moddedCreateSchema = z.object({
	runtime: runtimeSchema,
	initial_mods: z.array(modEntrySchema).default([]),
});

export const createServerRequestSchema = z.object({
	name: z
		.string()
		.regex(NAME_REGEX, "lowercase letters, digits, '-' (1-63 chars)"),
	mc_version: z.string().optional(),
	memory_mi: z.number().int().min(1024).max(16_384),
	cpu_millicores: z.number().int().min(250).max(16_000),
	exposure_mode: exposureModeSchema.optional(),
	storage_class: z.string().optional(),
	storage_size_gi: z.number().int().min(10).max(500).optional(),
	// B: server_type discriminator widens to 5; sub-configs for cf/modrinth/modded.
	server_type: sourceKindSchema.optional(),
	curseforge: curseforgeCreateSchema.optional(),
	modrinth: modrinthCreateSchema.optional(),
	modded: moddedCreateSchema.optional(),
});

export const createServerResponseSchema = z.object({
	id: z.string().uuid(),
	name: z.string(),
});

// --- restart --------------------------------------------------------------

const restartResponseSchema = z.object({
	id: z.string(),
	status: z.string(),
});

// --- modpack --------------------------------------------------------------

export const updateResolveResponseSchema = z.object({
	project_id: z.number().int().positive(),
	name: z.string(),
	slug: z.string(),
});

export const updateStartResponseSchema = z.object({
	status: z.string(),
	server_id: z.string(),
	target_version_id: z.string(),
});

export const autoUpdateModeSchema = z.enum(["never", "notify", "apply"]);

export const settingsRequestSchema = z.object({
	memory_mi: z.number().int().min(1024).max(16_384).optional(),
	cpu_millicores: z.number().int().min(250).max(16_000).optional(),
	auto_update_mode: autoUpdateModeSchema.optional(),
	version_skip: z.array(z.string()).optional(),
	force_version: z.string().nullable().optional(),
});

// --- inferred types -------------------------------------------------------

export type ServerSummary = z.infer<typeof serverSummarySchema>;
export type ServerDetail = z.infer<typeof serverDetailSchema>;
export type ClusterCapabilities = z.infer<typeof clusterCapabilitiesSchema>;
export type CreateServerRequest = z.infer<typeof createServerRequestSchema>;
export type ServerStatus = z.infer<typeof serverStatusSchema>;
export type ExposureMode = z.infer<typeof exposureModeSchema>;
export type Me = z.infer<typeof meSchema>;
export type SourceKind = z.infer<typeof sourceKindSchema>;
export type CfChannel = z.infer<typeof cfChannelSchema>;
export type AutoUpdateMode = z.infer<typeof autoUpdateModeSchema>;
export type SettingsRequest = z.infer<typeof settingsRequestSchema>;
export type UpdateResolveResponse = z.infer<typeof updateResolveResponseSchema>;
export type UpdateStartResponse = z.infer<typeof updateStartResponseSchema>;
export type McVersionsResponse = z.infer<typeof mcVersionsResponseSchema>;

// --- typed error wrapper --------------------------------------------------

/// Error thrown by every API function on a non-2xx response. `code` is
/// the stable kebab-case wire code from the backend; `message` is the
/// human-readable message.
export class ApiError extends Error {
	public readonly code: string;
	public readonly status: number;
	constructor(status: number, code: string, message: string) {
		super(message);
		this.name = "ApiError";
		this.code = code;
		this.status = status;
	}
}

// 401 from any /api/* call means our session cookie is missing/expired.
// Hand control to the backend's /api/auth/login → Authentik dance and never
// resolve the in-flight Promise so callers don't render an error state.
function redirectToLogin(): Promise<never> {
	if (typeof window !== "undefined") {
		window.location.replace("/api/auth/login");
	}
	return new Promise<never>(() => undefined);
}

async function jsonOrThrow<T>(
	res: Response,
	schema: z.ZodSchema<T>,
): Promise<T> {
	if (res.status === 401) return redirectToLogin();
	if (!res.ok) {
		const raw: unknown = await res.json().catch(() => ({}));
		const parsed = errorResponseSchema.safeParse(raw);
		if (parsed.success) {
			throw new ApiError(res.status, parsed.data.code, parsed.data.error);
		}
		throw new ApiError(
			res.status,
			"http_error",
			`${res.status.toString()} ${res.statusText}`,
		);
	}
	const json: unknown = await res.json();
	return schema.parse(json);
}

async function noContentOrThrow(res: Response): Promise<void> {
	if (res.status === 401) {
		await redirectToLogin();
		return;
	}
	if (res.ok) return;
	const raw: unknown = await res.json().catch(() => ({}));
	const parsed = errorResponseSchema.safeParse(raw);
	if (parsed.success) {
		throw new ApiError(res.status, parsed.data.code, parsed.data.error);
	}
	throw new ApiError(
		res.status,
		"http_error",
		`${res.status.toString()} ${res.statusText}`,
	);
}

// --- API functions --------------------------------------------------------

export async function fetchServers(
	signal: AbortSignal,
): Promise<readonly ServerSummary[]> {
	const res = await fetch("/api/servers", { signal });
	const body = await jsonOrThrow(res, serversResponseSchema);
	return body.servers;
}

export async function fetchServerDetail(
	id: string,
	signal: AbortSignal,
): Promise<ServerDetail> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}`, { signal });
	return jsonOrThrow(res, serverDetailSchema);
}

export async function fetchServerByName(
	name: string,
	signal: AbortSignal,
): Promise<ServerDetail> {
	const res = await fetch(`/api/servers/by-name/${encodeURIComponent(name)}`, {
		signal,
	});
	return jsonOrThrow(res, serverDetailSchema);
}

export async function fetchCapabilities(
	signal: AbortSignal,
): Promise<ClusterCapabilities> {
	const res = await fetch("/api/cluster/capabilities", { signal });
	return jsonOrThrow(res, clusterCapabilitiesSchema);
}

export async function fetchMcVersions(
	signal?: AbortSignal,
): Promise<McVersionsResponse> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch("/api/cluster/mc-versions", init);
	return jsonOrThrow(res, mcVersionsResponseSchema);
}

export async function fetchLogs(
	id: string,
	signal: AbortSignal,
): Promise<readonly string[]> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/logs`, {
		signal,
	});
	const body = await jsonOrThrow(res, logsResponseSchema);
	return body.lines;
}

export async function createServer(
	request: CreateServerRequest,
): Promise<{ id: string; name: string }> {
	const validated = createServerRequestSchema.parse(request);
	const res = await fetch("/api/servers", {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(validated),
	});
	return jsonOrThrow(res, createServerResponseSchema);
}

export async function startServer(id: string): Promise<ServerDetail> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/start`, {
		method: "POST",
	});
	return jsonOrThrow(res, serverDetailSchema);
}

export async function stopServer(id: string): Promise<ServerDetail> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/stop`, {
		method: "POST",
	});
	return jsonOrThrow(res, serverDetailSchema);
}

export async function restartServer(
	id: string,
): Promise<{ id: string; status: string }> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/restart`, {
		method: "POST",
	});
	return jsonOrThrow(res, restartResponseSchema);
}

export async function deleteServer(id: string): Promise<void> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}`, {
		method: "DELETE",
	});
	await noContentOrThrow(res);
}

export async function sendRconCommand(
	id: string,
	cmd: string,
): Promise<{ output: string }> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/rcon`, {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ cmd }),
	});
	return jsonOrThrow(res, rconResponseSchema);
}

export async function getMe(signal?: AbortSignal): Promise<Me> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch("/api/auth/me", init);
	return jsonOrThrow(res, meSchema);
}

export async function logout(): Promise<string> {
	const res = await fetch("/api/auth/logout", { method: "POST" });
	const body = await jsonOrThrow(res, logoutResponseSchema);
	return body.logoutUrl;
}

// --- modpack endpoints ----------------------------------------------------

/// Resolves a CurseForge URL slug to a project id via the backend (which
/// holds the API key — never exposed to the browser).
export async function resolveCurseForgeSlug(
	slug: string,
	signal?: AbortSignal,
): Promise<UpdateResolveResponse> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch(
		`/api/modpack/curseforge/resolve?slug=${encodeURIComponent(slug)}`,
		init,
	);
	return jsonOrThrow(res, updateResolveResponseSchema);
}

/// Kicks off an update for a modpack-backed server. `versionId` omits to use
/// the poller's cached latest.
export async function applyUpdate(
	id: string,
	versionId?: string,
): Promise<UpdateStartResponse> {
	const body =
		versionId !== undefined
			? JSON.stringify({ version_id: versionId })
			: JSON.stringify({});
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/update`, {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body,
	});
	return jsonOrThrow(res, updateStartResponseSchema);
}

/// PATCHes per-server modpack settings (auto_update_mode, version_skip,
/// force_version). Only the fields supplied are updated.
export async function updateServerSettings(
	id: string,
	patch: SettingsRequest,
): Promise<void> {
	const validated = settingsRequestSchema.parse(patch);
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/settings`, {
		method: "PATCH",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(validated),
	});
	await noContentOrThrow(res);
}

// --- catalog --------------------------------------------------------------

export const catalogProviderSchema = z.enum(["curseforge", "modrinth"]);

export const catalogHitSchema = z.object({
	provider: catalogProviderSchema,
	project_id: z.string(),
	slug: z.string(),
	name: z.string(),
	summary: z.string().default(""),
	icon_url: z.string().nullable(),
	downloads: z.number().int().nonnegative(),
	follows: z.number().int().nonnegative(),
	project_type: z.string(),
	loaders: z.array(z.string()).default([]),
	game_versions: z.array(z.string()).default([]),
	author: z.string().nullable().default(null),
	updated: z.string().default(""),
});

export const catalogSearchResponseSchema = z.object({
	results: z.array(catalogHitSchema),
});

export const catalogVersionSchema = z.object({
	version_id: z.string(),
	version_name: z.string(),
	channel: z.string(),
	loaders: z.array(z.string()).default([]),
	game_versions: z.array(z.string()).default([]),
	date_published: z.string(),
	primary_filename: z.string(),
	primary_url: z.string(),
	primary_sha512: z.string().nullable().default(null),
});

export const catalogVersionsResponseSchema = z.object({
	versions: z.array(catalogVersionSchema),
});

export type CatalogHit = z.infer<typeof catalogHitSchema>;
export type CatalogVersion = z.infer<typeof catalogVersionSchema>;
export type CatalogProvider = z.infer<typeof catalogProviderSchema>;
export type Runtime = z.infer<typeof runtimeSchema>;
export type ModEntry = z.infer<typeof modEntrySchema>;

export interface CatalogSearchParams {
	type: "mod" | "modpack";
	q: string;
	loader?: "fabric" | "forge" | "neoforge" | "paper";
	mc?: string;
	limit?: number;
	offset?: number;
}

/// Searches the unified catalog. Modpack queries hit CF + Modrinth; mod
/// queries hit Modrinth only.
export async function searchCatalog(
	params: CatalogSearchParams,
	signal?: AbortSignal,
): Promise<readonly CatalogHit[]> {
	const sp = new URLSearchParams({ type: params.type, q: params.q });
	if (params.loader !== undefined) sp.set("loader", params.loader);
	if (params.mc !== undefined) sp.set("mc", params.mc);
	if (params.limit !== undefined) sp.set("limit", params.limit.toString());
	if (params.offset !== undefined) sp.set("offset", params.offset.toString());
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch(`/api/catalog/search?${sp.toString()}`, init);
	const body = await jsonOrThrow(res, catalogSearchResponseSchema);
	return body.results;
}

/// Lists installable versions of one project, filtered by loader/mc.
export async function fetchCatalogVersions(
	provider: CatalogProvider,
	id: string,
	opts: { loader?: string; mc?: string } = {},
	signal?: AbortSignal,
): Promise<readonly CatalogVersion[]> {
	const sp = new URLSearchParams();
	if (opts.loader !== undefined) sp.set("loader", opts.loader);
	if (opts.mc !== undefined) sp.set("mc", opts.mc);
	const qs = sp.toString();
	const url = `/api/catalog/projects/${encodeURIComponent(provider)}/${encodeURIComponent(id)}/versions${qs.length > 0 ? `?${qs}` : ""}`;
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch(url, init);
	const body = await jsonOrThrow(res, catalogVersionsResponseSchema);
	return body.versions;
}

// --- modlist (modded servers) --------------------------------------------

export const modPendingOpSchema = z.discriminatedUnion("op", [
	z.object({ op: z.literal("add"), mod_entry: modEntrySchema }),
	z.object({ op: z.literal("remove"), filename: z.string() }),
	z.object({
		op: z.literal("bump"),
		filename: z.string(),
		to_version_id: z.string(),
		to_version_name: z.string(),
		to_filename: z.string(),
		to_download_url: z.string(),
		to_sha512: z.string().nullable().default(null),
	}),
]);

export const moddedConfigSchema = z.object({
	runtime: runtimeSchema,
	mc_version: z.string(),
	mods: z.array(modEntrySchema).default([]),
	pending: z.array(modPendingOpSchema).default([]),
});

export type ModPendingOp = z.infer<typeof modPendingOpSchema>;
export type ModdedConfig = z.infer<typeof moddedConfigSchema>;

export const modsApplyResponseSchema = z.object({
	status: z.string(),
	server_id: z.string(),
	pending_count: z.number().int().nonnegative(),
});

/// Appends a pending op to a modded server's modlist draft.
export async function addPendingMod(
	serverId: string,
	op: ModPendingOp,
): Promise<void> {
	const validated = modPendingOpSchema.parse(op);
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/mods`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(validated),
		},
	);
	await noContentOrThrow(res);
}

/// Drops a pending op by index.
export async function removePendingMod(
	serverId: string,
	idx: number,
): Promise<void> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/mods/pending/${idx.toString()}`,
		{ method: "DELETE" },
	);
	await noContentOrThrow(res);
}

/// Kicks the mod-sync FSM. WebSocket at /mods/apply/stream surfaces phases.
export async function applyMods(
	serverId: string,
): Promise<{ status: string; server_id: string; pending_count: number }> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/mods/apply`,
		{ method: "POST" },
	);
	return jsonOrThrow(res, modsApplyResponseSchema);
}
