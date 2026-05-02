-- Anvil v1 schema (spec §3).
--
-- Two tables: `servers` snapshots create-time config; `audit_log` records
-- mutating actions. Live runtime state (status, replicas, pod phase) is NOT
-- stored here — the k8s API is the source of truth (ADR 0001).

CREATE TABLE servers (
  name             TEXT PRIMARY KEY,
  mc_version       TEXT NOT NULL,
  memory_mb        INTEGER NOT NULL,
  storage_class    TEXT NOT NULL,            -- snapshotted at create time
  storage_size_gib INTEGER NOT NULL,         -- snapshotted at create time
  service_type     TEXT NOT NULL,            -- snapshotted at create time
  created_at       TEXT NOT NULL             -- RFC3339
);

CREATE TABLE audit_log (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  ts           TEXT NOT NULL,                -- RFC3339
  server_name  TEXT NOT NULL,                -- NOT a foreign key — survives server deletion
  action       TEXT NOT NULL,                -- created | started | stopped | edited | deleted
  details      TEXT,                         -- nullable JSON blob, action-specific
  actor        TEXT                          -- NULL until M4 (auth)
);

CREATE INDEX idx_audit_server_ts ON audit_log (server_name, ts DESC);
