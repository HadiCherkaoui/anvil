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

export const serverSummarySchema = z.object({
	id: z.string().uuid(),
	name: z.string(),
	status: serverStatusSchema,
	mc_version: z.string(),
	memory_mi: z.number().int().nonnegative(),
	exposure_mode: exposureModeSchema,
	endpoint: endpointSchema.nullable(),
	created_at: z.number().int(),
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
});

// --- capabilities ---------------------------------------------------------

export const clusterCapabilitiesSchema = z.object({
	loadbalancer: z.boolean(),
	nodeport: z.boolean(),
	clusterip: z.boolean(),
	available_storage_classes: z.array(z.string()),
	default_storage_class: z.string().nullable(),
});

// --- logs -----------------------------------------------------------------

export const logsResponseSchema = z.object({
	lines: z.array(z.string()),
});

// --- create ---------------------------------------------------------------

const NAME_REGEX = /^[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?$/;

export const createServerRequestSchema = z.object({
	name: z
		.string()
		.regex(NAME_REGEX, "lowercase letters, digits, '-' (1-63 chars)"),
	mc_version: z.string(),
	memory_mi: z.number().int().min(1024).max(16_384),
	exposure_mode: exposureModeSchema.optional(),
	storage_class: z.string().optional(),
	storage_size_gi: z.number().int().min(1).max(500).optional(),
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

// --- inferred types -------------------------------------------------------

export type ServerSummary = z.infer<typeof serverSummarySchema>;
export type ServerDetail = z.infer<typeof serverDetailSchema>;
export type ClusterCapabilities = z.infer<typeof clusterCapabilitiesSchema>;
export type CreateServerRequest = z.infer<typeof createServerRequestSchema>;
export type ServerStatus = z.infer<typeof serverStatusSchema>;
export type ExposureMode = z.infer<typeof exposureModeSchema>;

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

async function jsonOrThrow<T>(
	res: Response,
	schema: z.ZodSchema<T>,
): Promise<T> {
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
