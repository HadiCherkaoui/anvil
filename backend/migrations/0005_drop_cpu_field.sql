-- Drop the per-server CPU budget. The k8s container spec no longer carries
-- resource limits or requests (overprovisioning is intentional on the
-- homelab cluster), so the column has no consumer. Requires SQLite ≥ 3.35.

ALTER TABLE servers DROP COLUMN cpu_millicores;
