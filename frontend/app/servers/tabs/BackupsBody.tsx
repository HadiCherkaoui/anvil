"use client";

import {
	useCallback,
	useEffect,
	useMemo,
	useState,
	type ReactElement,
} from "react";

import {
	ApiError,
	createBackup,
	deleteBackup,
	fetchBackups,
	restoreBackup,
	type Backup,
} from "../../lib/api";
import { useServerDetail } from "../../lib/server-detail-context";

import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { Modal } from "../../components/Modal";
import { Skeleton } from "../../components/Skeleton";
import { useToast } from "../../components/Toast";
import { UpdateSheet } from "../../components/UpdateSheet";

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

function fmtBytes(n: number | null): string {
	if (n === null) return "—";
	let i = 0;
	let x = n;
	while (x >= 1024 && i < BYTE_UNITS.length - 1) {
		x /= 1024;
		i += 1;
	}
	return `${x.toFixed(1)} ${BYTE_UNITS[i] ?? "B"}`;
}

function fmtTs(s: number): string {
	return new Date(s * 1000).toLocaleString();
}

type Confirm = { kind: "restore" | "delete"; b: Backup };

type LoadState =
	| { kind: "loading" }
	| { kind: "ready"; backups: readonly Backup[] }
	| { kind: "error"; message: string };

export function BackupsBody(): ReactElement {
	const { detail, refresh } = useServerDetail();
	const toast = useToast();
	const [state, setState] = useState<LoadState>({ kind: "loading" });
	const [createOpen, setCreateOpen] = useState(false);
	const [createName, setCreateName] = useState("");
	const [confirm, setConfirm] = useState<Confirm | null>(null);
	const [progressOpen, setProgressOpen] = useState(false);

	const loadList = useCallback((): void => {
		fetchBackups(detail.id)
			.then((backups) => {
				setState({ kind: "ready", backups });
			})
			.catch((err: unknown) => {
				const message =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				setState({ kind: "error", message });
			});
	}, [detail.id]);

	useEffect(() => {
		loadList();
	}, [loadList]);

	const backups = state.kind === "ready" ? state.backups : [];
	const totalBytes = useMemo(
		() =>
			state.kind === "ready"
				? state.backups.reduce((acc, b) => acc + (b.size_bytes ?? 0), 0)
				: 0,
		[state],
	);

	const onCreate = (): void => {
		const trimmed = createName.trim();
		void createBackup(detail.id, trimmed === "" ? undefined : trimmed)
			.then(() => {
				setCreateOpen(false);
				setCreateName("");
				setProgressOpen(true);
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`backup failed · ${msg}`, "error");
			});
	};

	const onRestore = (b: Backup): void => {
		void restoreBackup(detail.id, b.id)
			.then(() => {
				setConfirm(null);
				setProgressOpen(true);
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`restore failed · ${msg}`, "error");
			});
	};

	const onDelete = (b: Backup): void => {
		void deleteBackup(detail.id, b.id)
			.then(() => {
				setConfirm(null);
				toast.push("backup deleted", "success");
				loadList();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`delete failed · ${msg}`, "error");
			});
	};

	const onProgressClose = (): void => {
		setProgressOpen(false);
		refresh();
		loadList();
	};

	return (
		<>
			<Card header="backups">
				<div className="flex items-center justify-between border-b border-border-soft pb-3">
					<span className="font-mono text-[12px] text-text-faint">
						{backups.length} backup{backups.length === 1 ? "" : "s"} ·{" "}
						{fmtBytes(totalBytes)}
					</span>
					<Button
						variant="primary"
						onClick={() => {
							setCreateOpen(true);
						}}
					>
						+ create backup
					</Button>
				</div>

				{state.kind === "loading" && (
					<div className="pt-3">
						<Skeleton variant="block" className="h-12" />
					</div>
				)}

				{state.kind === "error" && (
					<p className="pt-3 font-mono text-[12px] text-state-error">
						failed to load · {state.message}
					</p>
				)}

				{state.kind === "ready" && backups.length === 0 && (
					<p className="pt-3 font-mono text-[12px] text-text-faint">
						no backups yet — create one to capture this server&apos;s state.
					</p>
				)}

				{state.kind === "ready" && backups.length > 0 && (
					<ul className="divide-y divide-border-soft">
						{backups.map((b) => (
							<li
								key={b.id}
								className="grid grid-cols-[1fr_auto_auto_auto_auto] items-center gap-3 py-2 first:pt-3"
							>
								<span className="font-mono text-[12px] text-text-body">
									{b.name ?? "(unnamed)"}
								</span>
								<span className="font-mono text-[11px] text-text-faint">
									{fmtTs(b.created_at)}
								</span>
								<span className="font-mono text-[11px] text-text-faint">
									{b.mc_version}
								</span>
								<span className="font-mono text-[11px] text-text-faint">
									{fmtBytes(b.size_bytes)}
								</span>
								<span className="flex gap-2">
									<Button
										variant="ghost"
										onClick={() => {
											setConfirm({ kind: "restore", b });
										}}
									>
										restore
									</Button>
									<Button
										variant="danger"
										onClick={() => {
											setConfirm({ kind: "delete", b });
										}}
									>
										delete
									</Button>
								</span>
							</li>
						))}
					</ul>
				)}
			</Card>

			<Modal
				open={createOpen}
				onClose={() => {
					setCreateOpen(false);
				}}
				title="create backup"
				maxWidth="sm"
			>
				<label className="block font-mono text-[11px] uppercase tracking-wider text-text-muted">
					name (optional, ≤ 64 chars)
				</label>
				<input
					value={createName}
					onChange={(e) => {
						setCreateName(e.target.value);
					}}
					maxLength={64}
					placeholder="e.g. pre-1.21"
					className="mt-1 w-full rounded border border-border bg-bg px-2 py-1 font-mono text-[12px] text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
				/>
				<div className="mt-3 flex justify-end gap-2">
					<Button
						variant="secondary"
						onClick={() => {
							setCreateOpen(false);
						}}
					>
						cancel
					</Button>
					<Button variant="primary" onClick={onCreate}>
						create backup
					</Button>
				</div>
			</Modal>

			{confirm !== null && (
				<Modal
					open
					onClose={() => {
						setConfirm(null);
					}}
					title={`${confirm.kind} ${confirm.b.name ?? "(unnamed)"}?`}
					maxWidth="sm"
				>
					<p className="font-mono text-[12px] text-text-body">
						{confirm.kind === "restore"
							? "this stops the server, replaces world data and config with the snapshot, then restarts. on failure, the server may end in a mixed state — take another backup first if you want a safety net."
							: "delete this backup permanently?"}
					</p>
					<div className="mt-3 flex justify-end gap-2">
						<Button
							variant="secondary"
							onClick={() => {
								setConfirm(null);
							}}
						>
							cancel
						</Button>
						<Button
							variant={confirm.kind === "delete" ? "danger" : "primary"}
							onClick={() => {
								if (confirm.kind === "restore") onRestore(confirm.b);
								else onDelete(confirm.b);
							}}
						>
							{confirm.kind}
						</Button>
					</div>
				</Modal>
			)}

			<UpdateSheet
				serverId={detail.id}
				isOpen={progressOpen}
				onClose={onProgressClose}
			/>
		</>
	);
}
