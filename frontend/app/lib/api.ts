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

export const sourceKindSchema = z.enum(["vanilla", "curseforge"]);

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
	latest_changelog_excerpt: z.string().nullable().default(null),
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

export const createServerRequestSchema = z.object({
	name: z
		.string()
		.regex(NAME_REGEX, "lowercase letters, digits, '-' (1-63 chars)"),
	// Optional: omitted for CurseForge servers (the chosen file's display name
	// is stored as the version label by the backend).
	mc_version: z.string().optional(),
	memory_mi: z.number().int().min(1024).max(16_384),
	exposure_mode: exposureModeSchema.optional(),
	storage_class: z.string().optional(),
	storage_size_gi: z.number().int().min(1).max(500).optional(),
	// M5: server_type discriminator + sub-config for curseforge.
	server_type: sourceKindSchema.optional(),
	curseforge: curseforgeCreateSchema.optional(),
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
	target_version_id: z.number().int().positive(),
});

export const autoUpdateModeSchema = z.enum(["never", "notify", "apply"]);

export const settingsRequestSchema = z.object({
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

export async function fetchCapabilities(
	signal: AbortSignal,
): Promise<ClusterCapabilities> {
	const res = await fetch("/api/cluster/capabilities", { signal });
	return jsonOrThrow(res, clusterCapabilitiesSchema);
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

/// Kicks off an update for a CF-backed server. `versionId` omits to use the
/// poller's cached latest.
export async function applyUpdate(
	id: string,
	versionId?: number,
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
