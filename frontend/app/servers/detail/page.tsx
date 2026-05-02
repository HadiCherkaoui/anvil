"use client";

import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import {
	Suspense,
	useCallback,
	useEffect,
	useState,
	type ReactElement,
} from "react";

import { Button } from "../../components/Button";
import { ConfirmDeleteDialog } from "../../components/ConfirmDeleteDialog";
import { StatusBadge } from "../../components/StatusBadge";
import {
	ApiError,
	fetchLogs,
	fetchServerDetail,
	restartServer,
	startServer,
	stopServer,
	type ServerDetail,
} from "../../lib/api";

const DETAIL_POLL_MS = 5_000;
const LOG_POLL_MS = 15_000;

export default function ServerDetailPage(): ReactElement {
	return (
		<Suspense
			fallback={<p className="px-6 py-12 text-sm text-slate-400">loading…</p>}
		>
			<ServerDetail />
		</Suspense>
	);
}

function ServerDetail(): ReactElement {
	const router = useRouter();
	const params = useSearchParams();
	const id = params.get("id");

	const [detail, setDetail] = useState<ServerDetail | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [logs, setLogs] = useState<readonly string[]>([]);
	const [actionError, setActionError] = useState<string | null>(null);
	const [busy, setBusy] = useState(false);
	const [confirmOpen, setConfirmOpen] = useState(false);

	const reloadDetail = useCallback(
		async (signal: AbortSignal): Promise<void> => {
			if (id === null) return;
			try {
				const d = await fetchServerDetail(id, signal);
				setDetail(d);
				setError(null);
			} catch (err: unknown) {
				if (err instanceof DOMException && err.name === "AbortError") return;
				const message =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				setError(message);
			}
		},
		[id],
	);

	const reloadLogs = useCallback(async (): Promise<void> => {
		if (id === null) return;
		const ctrl = new AbortController();
		try {
			const lines = await fetchLogs(id, ctrl.signal);
			setLogs(lines);
		} catch (err: unknown) {
			void err; // log fetch failures are non-fatal — leave existing tail
		}
	}, [id]);

	useEffect(() => {
		if (id === null) return undefined;
		const ctrl = new AbortController();
		// eslint-disable-next-line react-hooks/set-state-in-effect
		void reloadDetail(ctrl.signal);
		const handle = setInterval(() => {
			void reloadDetail(ctrl.signal);
		}, DETAIL_POLL_MS);
		return () => {
			clearInterval(handle);
			ctrl.abort();
		};
	}, [id, reloadDetail]);

	useEffect(() => {
		if (id === null || detail?.status !== "running") return undefined;
		// eslint-disable-next-line react-hooks/set-state-in-effect
		void reloadLogs();
		const handle = setInterval(() => {
			void reloadLogs();
		}, LOG_POLL_MS);
		return () => {
			clearInterval(handle);
		};
	}, [id, detail?.status, reloadLogs]);

	const runAction = useCallback(
		(fn: () => Promise<unknown>): void => {
			setActionError(null);
			setBusy(true);
			fn()
				.then(() => {
					const ctrl = new AbortController();
					void reloadDetail(ctrl.signal);
				})
				.catch((err: unknown) => {
					if (err instanceof ApiError) {
						setActionError(`${err.code}: ${err.message}`);
					} else {
						setActionError(
							err instanceof Error ? err.message : "unknown error",
						);
					}
				})
				.finally(() => {
					setBusy(false);
				});
		},
		[reloadDetail],
	);

	if (id === null) {
		return (
			<main className="min-h-screen px-6 py-12">
				<p className="text-sm text-red-400">missing id query param</p>
				<BackLink />
			</main>
		);
	}

	if (error !== null && detail === null) {
		return (
			<main className="min-h-screen px-6 py-12">
				<p className="text-sm text-red-400">failed to load: {error}</p>
				<BackLink />
			</main>
		);
	}

	if (detail === null) {
		return (
			<main className="min-h-screen px-6 py-12">
				<p className="text-sm text-slate-400">loading…</p>
			</main>
		);
	}

	const canStart = detail.status === "stopped";
	const canStop = detail.status === "running" || detail.status === "starting";
	const canRestart = detail.status === "running";
	const canDelete = detail.status === "stopped";

	const endpointDisplay =
		detail.endpoint === null
			? "address pending…"
			: `${detail.endpoint.host}:${detail.endpoint.port.toString()}`;

	return (
		<main className="min-h-screen px-6 py-12">
			<div className="mx-auto flex max-w-4xl flex-col gap-8">
				<BackLink />
				<header className="flex flex-wrap items-start justify-between gap-4">
					<div className="flex flex-col gap-2">
						<h1 className="font-mono text-2xl font-semibold tracking-tight">
							{detail.name}
						</h1>
						<div className="flex items-center gap-3 text-xs text-slate-400">
							<StatusBadge status={detail.status} />
							<span className="font-mono">{detail.mc_version}</span>
							<span>·</span>
							<span>{(detail.memory_mi / 1024).toString()} GiB</span>
						</div>
					</div>
					<div className="flex flex-wrap items-center gap-2">
						<Button
							variant="primary"
							disabled={busy || !canStart}
							onClick={() => {
								runAction(() => startServer(id));
							}}
						>
							start
						</Button>
						<Button
							variant="secondary"
							disabled={busy || !canStop}
							onClick={() => {
								runAction(() => stopServer(id));
							}}
						>
							stop
						</Button>
						<Button
							variant="secondary"
							disabled={busy || !canRestart}
							onClick={() => {
								runAction(() => restartServer(id));
							}}
						>
							restart
						</Button>
						<Button
							variant="danger"
							disabled={busy || !canDelete}
							onClick={() => {
								setConfirmOpen(true);
							}}
						>
							delete
						</Button>
					</div>
				</header>

				{actionError !== null && (
					<p className="text-sm text-red-400">{actionError}</p>
				)}

				<section className="flex flex-col gap-2 rounded-lg border border-slate-800 p-4">
					<h2 className="text-xs uppercase tracking-wide text-slate-400">
						Connect
					</h2>
					<p className="font-mono text-base text-slate-100">
						{endpointDisplay}
					</p>
				</section>

				<section className="flex flex-col gap-3 rounded-lg border border-slate-800 p-4">
					<div className="flex items-baseline justify-between">
						<h2 className="text-xs uppercase tracking-wide text-slate-400">
							Recent logs
						</h2>
						<button
							type="button"
							onClick={() => {
								void reloadLogs();
							}}
							className="text-xs text-slate-400 hover:text-slate-100"
						>
							refresh
						</button>
					</div>
					<pre className="max-h-96 overflow-auto rounded-md bg-slate-950 p-3 font-mono text-xs leading-relaxed text-slate-300">
						{logs.length === 0 ? "(no logs yet)" : logs.join("\n")}
					</pre>
				</section>

				<section className="grid grid-cols-2 gap-4 rounded-lg border border-slate-800 p-4 text-sm sm:grid-cols-3">
					<DetailField label="Server type" value={detail.server_type} />
					<DetailField label="Exposure" value={detail.exposure_mode} />
					<DetailField
						label="NodePort"
						value={detail.nodeport === null ? "—" : detail.nodeport.toString()}
					/>
					<DetailField
						label="Storage class"
						value={detail.storage_class ?? "(cluster default)"}
					/>
					<DetailField
						label="Storage size"
						value={`${detail.storage_size_gi.toString()} GiB`}
					/>
					<DetailField label="Created" value={formatTs(detail.created_at)} />
					<DetailField
						label="Last started"
						value={
							detail.last_started_at === null
								? "—"
								: formatTs(detail.last_started_at)
						}
					/>
				</section>
			</div>

			{confirmOpen && (
				<ConfirmDeleteDialog
					open={confirmOpen}
					onClose={() => {
						setConfirmOpen(false);
					}}
					serverId={id}
					serverName={detail.name}
					onDeleted={() => {
						setConfirmOpen(false);
						router.push("/");
					}}
				/>
			)}
		</main>
	);
}

interface DetailFieldProps {
	label: string;
	value: string;
}

function DetailField({ label, value }: DetailFieldProps): ReactElement {
	return (
		<div className="flex flex-col gap-1">
			<span className="text-xs uppercase tracking-wide text-slate-400">
				{label}
			</span>
			<span className="font-mono text-slate-200">{value}</span>
		</div>
	);
}

function BackLink(): ReactElement {
	return (
		<Link
			href="/"
			className="inline-flex w-fit items-center gap-1 text-xs text-slate-400 hover:text-slate-100"
		>
			← back to all servers
		</Link>
	);
}

function formatTs(unixSeconds: number): string {
	return new Date(unixSeconds * 1000)
		.toISOString()
		.replace("T", " ")
		.slice(0, 19);
}
