// Friendly translations for the kebab-case `EndReason` strings the
// /logs/stream WS emits. Audit §1.5 — users were seeing internal
// protocol vocabulary like "pod-unavailable" interpolated into the UI.

const FRIENDLY: Record<string, string> = {
	"pod-unavailable": "the server's pod went away",
	"client-closed": "log stream closed",
	"server-shutdown": "panel restarted",
	connecting: "still connecting",
	reconnecting: "reconnecting…",
	"stream-closed": "log stream closed",
	"stream-error": "log stream error",
};

export function friendlyEndReason(reason: string | null | undefined): string {
	if (reason === null || reason === undefined || reason === "") return "ended";
	return FRIENDLY[reason] ?? reason;
}
