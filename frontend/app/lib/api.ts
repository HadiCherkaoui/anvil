// Network-boundary types for the panel API. Every shape is validated with
// Zod at runtime so a backend-deploy drift is caught immediately rather
// than silently rendering garbage.

import { z } from "zod";

const endpointSchema = z.object({
  host: z.string(),
  port: z.number().int().nonnegative(),
});

const serverStatusSchema = z.enum([
  "running",
  "stopped",
  "starting",
  "stopping",
  "error",
]);

export const serverSummarySchema = z.object({
  name: z.string(),
  status: serverStatusSchema,
  mc_version: z.string(),
  memory_mb: z.number().int().nonnegative(),
  endpoint: endpointSchema.nullable(),
  created_at: z.string(),
});

export const serversResponseSchema = z.object({
  servers: z.array(serverSummarySchema),
});

export type ServerSummary = z.infer<typeof serverSummarySchema>;

export async function fetchServers(signal: AbortSignal): Promise<ServerSummary[]> {
  const res = await fetch("/api/servers", { signal });
  if (!res.ok) {
    throw new Error(`GET /api/servers returned ${res.status.toString()}`);
  }
  const json: unknown = await res.json();
  return serversResponseSchema.parse(json).servers;
}
