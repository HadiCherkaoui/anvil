-- Per-server tunable Minecraft server.properties values, applied via itzg's
-- env-vars-to-server.properties overlay on every pod start. JSON object
-- keyed by server.properties field name; empty `{}` decodes to vanilla
-- defaults via the typed struct's `#[serde(default)]` per-field.

ALTER TABLE servers ADD COLUMN properties TEXT NOT NULL DEFAULT '{}';
