// Friendly translations for the kebab-case `EndReason` strings the
// /logs/stream WS emits — without them the UI shows internal protocol
// vocabulary like "pod-unavailable".

const FRIENDLY: Record<string, string> = {
	"pod-unavailable": "the server's pod went away",
	"client-closed": "log stream closed",
	"server-shutdown": "panel restarted",
};

export function friendlyEndReason(reason: string | null | undefined): string {
	if (reason === null || reason === undefined || reason === "") return "ended";
	return FRIENDLY[reason] ?? reason;
}
