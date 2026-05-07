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

// --- server.properties subset --------------------------------------------

export const difficultySchema = z.enum(["peaceful", "easy", "normal", "hard"]);
export const gamemodeSchema = z.enum([
	"survival",
	"creative",
	"adventure",
	"spectator",
]);

export const serverPropertiesSchema = z.object({
	difficulty: difficultySchema.default("normal"),
	hardcore: z.boolean().default(false),
	gamemode: gamemodeSchema.default("survival"),
	force_gamemode: z.boolean().default(false),
	max_players: z.number().int().min(1).max(200).default(20),
	view_distance: z.number().int().min(3).max(32).default(10),
	simulation_distance: z.number().int().min(3).max(32).default(10),
	pvp: z.boolean().default(true),
	white_list: z.boolean().default(false),
	spawn_protection: z.number().int().min(0).max(256).default(16),
	spawn_animals: z.boolean().default(true),
	spawn_monsters: z.boolean().default(true),
	spawn_npcs: z.boolean().default(true),
	allow_flight: z.boolean().default(false),
	allow_nether: z.boolean().default(true),
	enable_command_block: z.boolean().default(false),
	// itzg `SEED` -> vanilla `level-seed`. Empty string = random world.
	// Capped at 256 chars and must not contain control chars (matches the
	// backend `properties_seed_invalid` validation).
	seed: z
		.string()
		.max(256)
		.refine(
			(s) => {
				for (let i = 0; i < s.length; i++) {
					const code = s.charCodeAt(i);
					if (code < 0x20 || code === 0x7f) return false;
				}
				return true;
			},
			{ message: "seed must not contain control characters" },
		)
		.default(""),
});

export type Difficulty = z.infer<typeof difficultySchema>;
export type Gamemode = z.infer<typeof gamemodeSchema>;
export type ServerProperties = z.infer<typeof serverPropertiesSchema>;

export const DEFAULT_PROPERTIES: ServerProperties =
	serverPropertiesSchema.parse({});

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

export const modUpdateInfoSchema = z.object({
	provider: z.string(),
	project_id: z.string(),
	current_version_id: z.string(),
	latest_version_id: z.string(),
	latest_version_name: z.string(),
});

export type ModUpdateInfo = z.infer<typeof modUpdateInfoSchema>;

export const loaderUpdateInfoSchema = z.object({
	current_loader: z.string(),
	latest_loader: z.string(),
});

export type LoaderUpdateInfo = z.infer<typeof loaderUpdateInfoSchema>;

export const serverDetailSchema = serverSummarySchema.extend({
	storage_class: z.string().nullable(),
	storage_size_gi: z.number().int().nonnegative(),
	nodeport: z.number().int().nullable(),
	last_started_at: z.number().int().nullable(),
	// M5: parsed provider config (passthrough JSON value) and the latest
	// cached upstream version, when any.
	source_config: z.unknown().default(null),
	latest_version_id: z.number().int().nullable().default(null),
	// True when the per-server file-helper Pod (`mc-{id}-files`) is up and
	// not mid-deletion. The Files tab uses this to show a manual kill bar
	// when the server is stopped but the helper is still running.
	files_helper_running: z.boolean().default(false),
	// Per-mod / per-plugin updates the poller has detected.
	mod_updates: z.array(modUpdateInfoSchema).default([]),
	// Newer Forge / NeoForge loader version available for this server's
	// MC version. `null` for fabric / paper / vanilla / modpack-driven
	// servers and for forge/neoforge servers already on the latest.
	loader_update: loaderUpdateInfoSchema.nullable().default(null),
	// User-tunable subset of server.properties. Defaults applied when the
	// backend hasn't been redeployed with the column yet.
	properties: serverPropertiesSchema.default(DEFAULT_PROPERTIES),
});

// --- capabilities ---------------------------------------------------------

export const clusterCapabilitiesSchema = z.object({
	loadbalancer: z.boolean(),
	nodeport: z.boolean(),
	clusterip: z.boolean(),
	available_storage_classes: z.array(z.string()),
	// Subset of available_storage_classes whose `allowVolumeExpansion` is
	// true; the storage-resize control in Settings is gated on the server's
	// SC being in this list.
	expandable_storage_classes: z.array(z.string()).default([]),
	default_storage_class: z.string().nullable(),
	// M5: gates the CurseForge option in the New Server modal.
	cf_api_key_present: z.boolean().default(false),
});

export const mcVersionsResponseSchema = z.object({
	versions: z.array(z.string()).min(1),
	source: z.enum(["mojang", "fallback"]),
});

export const paperVersionsResponseSchema = z.object({
	versions: z.array(z.string()).min(1),
	source: z.enum(["papermc", "fallback"]),
});

export type PaperVersionsResponse = z.infer<typeof paperVersionsResponseSchema>;

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
	loader_version: z.string().min(1).optional(),
});

export const paperCreateSchema = z.object({
	initial_plugins: z.array(modEntrySchema).default([]),
});

export const createServerRequestSchema = z.object({
	name: z
		.string()
		.regex(NAME_REGEX, "lowercase letters, digits, '-' (1-63 chars)"),
	mc_version: z.string().optional(),
	memory_mi: z.number().int().min(1024).max(65_536),
	exposure_mode: exposureModeSchema.optional(),
	storage_class: z.string().optional(),
	storage_size_gi: z.number().int().min(10).max(500).optional(),
	// Provider discriminator (vanilla | curseforge | modrinth | modded | paper);
	// when modpack/modded, the matching sub-config below is required.
	source_kind: sourceKindSchema.optional(),
	curseforge: curseforgeCreateSchema.optional(),
	modrinth: modrinthCreateSchema.optional(),
	modded: moddedCreateSchema.optional(),
	paper: paperCreateSchema.optional(),
	properties: serverPropertiesSchema.optional(),
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

export const updateStartResponseSchema = z.object({
	status: z.string(),
	server_id: z.string(),
	target_version_id: z.string(),
});

export const autoUpdateModeSchema = z.enum(["never", "notify", "apply"]);

export const settingsRequestSchema = z.object({
	memory_mi: z.number().int().min(1024).max(65_536).optional(),
	auto_update_mode: autoUpdateModeSchema.optional(),
	version_skip: z.array(z.string()).optional(),
	force_version: z.string().nullable().optional(),
	properties: serverPropertiesSchema.optional(),
});

// --- loader versions (Forge / NeoForge maven-metadata) -------------------

export const loaderVersionsSchema = z.object({
	mc_versions: z.array(z.string()),
	by_mc: z.record(z.string(), z.array(z.string())),
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
export type UpdateStartResponse = z.infer<typeof updateStartResponseSchema>;
export type McVersionsResponse = z.infer<typeof mcVersionsResponseSchema>;
export type LoaderVersions = z.infer<typeof loaderVersionsSchema>;

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

const resizeStorageResponseSchema = z.object({
	size_gi: z.number().int().positive(),
});

/// PATCHes the server's data PVC to a larger size_gi. Grow only — backend
/// rejects shrink with 400 `shrink_unsupported`. Filesystem expansion is async
/// (CSI), so the displayed size only reflects after the next detail fetch.
export async function resizeServerStorage(
	id: string,
	sizeGi: number,
): Promise<{ size_gi: number }> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/storage`, {
		method: "PATCH",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ size_gi: sizeGi }),
	});
	return jsonOrThrow(res, resizeStorageResponseSchema);
}

/// DELETEs the per-server file-helper Pod. 204 when the helper existed and
/// was torn down; 200 with `{ already_gone: true }` when no helper Pod was
/// present. 409 `helper_unsafe_to_kill` when the server is running.
export async function killFilesHelper(id: string): Promise<void> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(id)}/files/helper`,
		{ method: "DELETE" },
	);
	if (res.status === 200) {
		// Already-gone is success.
		return;
	}
	await noContentOrThrow(res);
}

export async function fetchMcVersions(
	signal?: AbortSignal,
): Promise<McVersionsResponse> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch("/api/cluster/mc-versions", init);
	return jsonOrThrow(res, mcVersionsResponseSchema);
}

/// Fetches the Paper-supported MC version list (PaperMC API). Used by
/// the Paper create flow so the dropdown only offers versions itzg
/// won't reject at boot.
export async function fetchPaperVersions(
	signal?: AbortSignal,
): Promise<PaperVersionsResponse> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch("/api/papermc/versions", init);
	return jsonOrThrow(res, paperVersionsResponseSchema);
}

export async function fetchLoaderVersions(
	runtime: "forge" | "neoforge",
	signal?: AbortSignal,
): Promise<LoaderVersions> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch(`/api/runtimes/${runtime}/versions`, init);
	return jsonOrThrow(res, loaderVersionsSchema);
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

// --- metrics --------------------------------------------------------------

export const serverMetricsSchema = z.object({
	cpu_millicores: z.number().int().nonnegative().nullable(),
	memory_mi: z.number().int().nonnegative().nullable(),
});

export type ServerMetrics = z.infer<typeof serverMetricsSchema>;

/// Fetches live CPU/memory from metrics-server. Both fields can be null when
/// the metrics API isn't installed or hasn't scraped this pod yet.
export async function fetchServerMetrics(
	id: string,
	signal?: AbortSignal,
): Promise<ServerMetrics> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch(
		`/api/servers/${encodeURIComponent(id)}/metrics`,
		init,
	);
	return jsonOrThrow(res, serverMetricsSchema);
}

// --- modpack endpoints ----------------------------------------------------

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

/// PATCHes the MC version (and optional loader version for modded servers).
/// Returns 202 when the version-change FSM is started; the frontend reuses
/// the existing /update/stream WS to render phase progress.
export const changeVersionResponseSchema = z.object({
	status: z.string(),
	server_id: z.string(),
});

export type ChangeVersionResponse = z.infer<typeof changeVersionResponseSchema>;

export async function changeServerVersion(
	id: string,
	body: { mc_version: string; loader_version?: string },
): Promise<ChangeVersionResponse> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/version`, {
		method: "PATCH",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(body),
	});
	return jsonOrThrow(res, changeVersionResponseSchema);
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
	type: "mod" | "modpack" | "plugin";
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

export const addPendingResponseSchema = z.object({
	added: z.array(modEntrySchema).default([]),
	added_count: z.number().int().nonnegative(),
});

export type AddPendingResponse = z.infer<typeof addPendingResponseSchema>;

/// Appends a pending op to a modded server's modlist draft. The backend
/// resolves required dependencies of an Add op upstream and folds them
/// into the same response — `added` lists every mod that landed in
/// `pending` from this call (seed + transitive deps).
export async function addPendingMod(
	serverId: string,
	op: ModPendingOp,
): Promise<AddPendingResponse> {
	const validated = modPendingOpSchema.parse(op);
	const res = await fetch(`/api/servers/${encodeURIComponent(serverId)}/mods`, {
		method: "POST",
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(validated),
	});
	return jsonOrThrow(res, addPendingResponseSchema);
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

// --- plugins (paper servers) ---------------------------------------------

export const paperConfigSchema = z.object({
	mc_version: z.string(),
	paper_build: z.string().nullable().default(null),
	plugins: z.array(modEntrySchema).default([]),
	pending_plugins: z.array(modEntrySchema).default([]),
});

export const pluginsListResponseSchema = z.object({
	plugins: z.array(modEntrySchema),
	pending_plugins: z.array(modEntrySchema),
});

export const pluginsApplyResponseSchema = z.object({
	status: z.string(),
	server_id: z.string(),
	pending_count: z.number().int().nonnegative(),
});

export type PaperConfig = z.infer<typeof paperConfigSchema>;
export type PluginsListResponse = z.infer<typeof pluginsListResponseSchema>;

/// Lists committed and pending plugins for a Paper server.
export async function listServerPlugins(
	serverId: string,
	signal?: AbortSignal,
): Promise<PluginsListResponse> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/plugins`,
		init,
	);
	return jsonOrThrow(res, pluginsListResponseSchema);
}

/// Stages adding a plugin to a Paper server's pending list. The backend
/// resolves required dependencies upstream and appends them too — `added`
/// lists every plugin that landed in pending (seed + transitive deps).
export async function addServerPlugin(
	serverId: string,
	entry: ModEntry,
): Promise<AddPendingResponse> {
	const validated = modEntrySchema.parse(entry);
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/plugins`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(validated),
		},
	);
	return jsonOrThrow(res, addPendingResponseSchema);
}

/// Stages removing a plugin from a Paper server's pending list.
export async function removeServerPlugin(
	serverId: string,
	filename: string,
): Promise<void> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/plugins/${encodeURIComponent(
			filename,
		)}`,
		{ method: "DELETE" },
	);
	await noContentOrThrow(res);
}

/// Kicks the plugin-sync FSM. WebSocket at /plugins/apply/stream surfaces phases.
export async function applyServerPlugins(
	serverId: string,
): Promise<{ status: string; server_id: string; pending_count: number }> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/plugins/apply`,
		{ method: "POST" },
	);
	return jsonOrThrow(res, pluginsApplyResponseSchema);
}

// --- players (sub-project C) --------------------------------------------------

export const onlinePlayersSchema = z.object({
	count: z.number().int().nonnegative(),
	max: z.number().int().nonnegative(),
	players: z.array(z.string()),
});

export const banEntrySchema = z.object({
	name: z.string(),
	reason: z.string(),
});

export const banIpEntrySchema = z.object({
	ip: z.string(),
	reason: z.string(),
});

export const playerEventSchema = z.object({
	kind: z.enum(["joined", "left"]),
	player: z.string(),
	ts_ms: z.number().int(),
});

export const playersResponseSchema = z.object({
	online: onlinePlayersSchema,
	whitelist: z.array(z.string()),
	banlist: z.object({
		players: z.array(banEntrySchema),
		ips: z.array(banIpEntrySchema),
	}),
	history: z.array(playerEventSchema),
});

export const playerActionSchema = z.discriminatedUnion("action", [
	z.object({
		action: z.literal("kick"),
		player: z.string(),
		reason: z.string().optional(),
	}),
	z.object({
		action: z.literal("ban"),
		player: z.string(),
		reason: z.string().optional(),
	}),
	z.object({
		action: z.literal("ban-ip"),
		player: z.string(),
		reason: z.string().optional(),
	}),
	z.object({ action: z.literal("pardon"), player: z.string() }),
	z.object({ action: z.literal("pardon-ip"), ip: z.string() }),
	z.object({ action: z.literal("op"), player: z.string() }),
	z.object({ action: z.literal("deop"), player: z.string() }),
	z.object({
		action: z.literal("gamemode"),
		player: z.string(),
		mode: gamemodeSchema,
	}),
	z.object({
		action: z.literal("tell"),
		player: z.string(),
		message: z.string(),
	}),
	z.object({ action: z.literal("whitelist-add"), player: z.string() }),
	z.object({ action: z.literal("whitelist-remove"), player: z.string() }),
]);

export type PlayersResponse = z.infer<typeof playersResponseSchema>;
export type PlayerEvent = z.infer<typeof playerEventSchema>;
export type BanEntry = z.infer<typeof banEntrySchema>;
export type BanIpEntry = z.infer<typeof banIpEntrySchema>;
export type PlayerAction = z.infer<typeof playerActionSchema>;

/// Fetches the bulk Players response. 409 on stopped server is
/// surfaced as a typed ApiError (`code: "server_not_running"`).
export async function fetchPlayers(
	id: string,
	signal: AbortSignal,
): Promise<PlayersResponse> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/players`, {
		signal,
	});
	return jsonOrThrow(res, playersResponseSchema);
}

/// Runs one player action. 204 on success.
export async function runPlayerAction(
	id: string,
	action: PlayerAction,
): Promise<void> {
	const validated = playerActionSchema.parse(action);
	const res = await fetch(
		`/api/servers/${encodeURIComponent(id)}/players/action`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(validated),
		},
	);
	await noContentOrThrow(res);
}

/// Sends `say <message>` to the server.
export async function broadcastMessage(
	id: string,
	message: string,
): Promise<void> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(id)}/players/broadcast`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ message }),
		},
	);
	await noContentOrThrow(res);
}

// --- backups (Spec 5) ----------------------------------------------------

export const backupKindSchema = z.enum(["manual", "auto"]);

export const backupSchema = z.object({
	id: z.string(),
	name: z.string().nullable(),
	created_at: z.number().int(),
	mc_version: z.string(),
	size_bytes: z.number().int().nullable(),
	// Migration 0010 rows: `manual` for user-triggered, `auto` for the
	// modpack-update / mc-version-change orchestrators. Default keeps the
	// schema tolerant if the backend predates the migration.
	kind: backupKindSchema.default("manual"),
	reason: z.string().nullable().default(null),
});

export type BackupKind = z.infer<typeof backupKindSchema>;

export type Backup = z.infer<typeof backupSchema>;

const backupListSchema = z.array(backupSchema);

const createBackupResponseSchema = z.object({
	status: z.string(),
	backup_id: z.string(),
});

const startedResponseSchema = z.object({ status: z.string() });

/// Lists manual backups for a server, newest first.
export async function fetchBackups(
	serverId: string,
	signal?: AbortSignal,
): Promise<readonly Backup[]> {
	const init: RequestInit = signal ? { signal } : {};
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/backups`,
		init,
	);
	return jsonOrThrow(res, backupListSchema);
}

/// Kicks off a new manual backup. The /update/stream WS surfaces phases.
export async function createBackup(
	serverId: string,
	name?: string,
): Promise<{ status: string; backup_id: string }> {
	const body =
		name !== undefined && name.length > 0
			? JSON.stringify({ name })
			: JSON.stringify({});
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/backups`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body,
		},
	);
	return jsonOrThrow(res, createBackupResponseSchema);
}

/// Starts a restore from a manual backup. Reuses /update/stream for phases.
export async function restoreBackup(
	serverId: string,
	backupId: string,
): Promise<{ status: string }> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/backups/${encodeURIComponent(backupId)}/restore`,
		{ method: "POST" },
	);
	return jsonOrThrow(res, startedResponseSchema);
}

/// Synchronously deletes a manual backup (the Job runs + waits server-side;
/// `rm` over a mounted PVC is sub-second). 204 on success.
export async function deleteBackup(
	serverId: string,
	backupId: string,
): Promise<void> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/backups/${encodeURIComponent(backupId)}`,
		{ method: "DELETE" },
	);
	await noContentOrThrow(res);
}

// --- Sub-project D: file browser ----------------------------------------

export const fileEntryTypeSchema = z.enum(["f", "d", "l", "o"]);
export type FileEntryType = z.infer<typeof fileEntryTypeSchema>;

export const fileEntrySchema = z.object({
	name: z.string().min(1),
	type: fileEntryTypeSchema,
	size: z.number().nonnegative(),
	mtime: z.number(),
});
export type FileEntry = z.infer<typeof fileEntrySchema>;

export const fileListResponseSchema = z.object({
	path: z.string().startsWith("/"),
	entries: z.array(fileEntrySchema),
});
export type FileListResponse = z.infer<typeof fileListResponseSchema>;

export const fileActionSchema = z.discriminatedUnion("action", [
	z.object({ action: z.literal("mkdir"), path: z.string().min(1) }),
	z.object({
		action: z.literal("rename"),
		from: z.string().min(1),
		to: z.string().min(1),
	}),
	z.object({
		action: z.literal("delete"),
		path: z.string().min(1),
		recursive: z.boolean(),
	}),
]);
export type FileAction = z.infer<typeof fileActionSchema>;

/// Lists entries under `path` for `serverId`. Backend lazy-spawns the
/// helper Pod when the server is stopped, so the first call after a
/// stop may take 5–15 s; the `useFiles` hook surfaces this as the
/// "warming" status.
export async function fetchFileList(
	serverId: string,
	path: string,
	signal: AbortSignal,
): Promise<FileListResponse> {
	const url = `/api/servers/${encodeURIComponent(serverId)}/files?path=${encodeURIComponent(path)}`;
	const res = await fetch(url, { signal });
	return jsonOrThrow(res, fileListResponseSchema);
}

/// Returns the URL to issue a GET against to download the file.
/// Used by `<a download>` and progressive enhancement; not all callers
/// need to invoke fetch directly.
export function downloadFileUrl(serverId: string, path: string): string {
	return `/api/servers/${encodeURIComponent(serverId)}/files/raw?path=${encodeURIComponent(path)}`;
}

/// Streams a file body to the backend with progress events. Uses
/// XMLHttpRequest because `fetch` does not expose `upload.onprogress`.
export async function uploadFile(
	serverId: string,
	path: string,
	blob: Blob,
	opts: { onProgress?: (frac: number) => void; signal?: AbortSignal } = {},
): Promise<void> {
	return new Promise<void>((resolve, reject) => {
		const xhr = new XMLHttpRequest();
		const url = `/api/servers/${encodeURIComponent(serverId)}/files?path=${encodeURIComponent(path)}`;
		xhr.open("PUT", url);
		xhr.responseType = "json";
		xhr.upload.onprogress = (e): void => {
			if (opts.onProgress && e.lengthComputable) {
				opts.onProgress(e.loaded / e.total);
			}
		};
		xhr.onload = (): void => {
			if (xhr.status === 401) {
				if (typeof window !== "undefined") {
					window.location.replace("/api/auth/login");
				}
				reject(new ApiError(401, "unauthorized", "redirecting to login"));
				return;
			}
			if (xhr.status === 204) {
				resolve();
				return;
			}
			const body = xhr.response as { error?: string; code?: string } | null;
			reject(
				new ApiError(
					xhr.status,
					body?.code ?? "unknown",
					body?.error ?? xhr.statusText,
				),
			);
		};
		xhr.onerror = (): void => {
			reject(new ApiError(0, "network", "network error during upload"));
		};
		xhr.onabort = (): void => {
			reject(new ApiError(0, "aborted", "upload cancelled"));
		};
		if (opts.signal) {
			const onAbort = (): void => {
				xhr.abort();
			};
			opts.signal.addEventListener("abort", onAbort, { once: true });
		}
		xhr.send(blob);
	});
}

/// Runs one file action (mkdir, rename, delete). 204 on success.
export async function runFileAction(
	serverId: string,
	action: FileAction,
): Promise<void> {
	const validated = fileActionSchema.parse(action);
	const res = await fetch(
		`/api/servers/${encodeURIComponent(serverId)}/files/action`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(validated),
		},
	);
	await noContentOrThrow(res);
}
