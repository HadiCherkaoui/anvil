"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useState, type ReactElement } from "react";

import {
	ApiError,
	downloadFileUrl,
	killFilesHelper,
	runFileAction,
	startServer,
	type FileEntry,
} from "../../lib/api";
import { useServerDetail } from "../../lib/server-detail-context";
import { useFiles } from "../../lib/use-files";

import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { ConfirmDeleteDialog } from "../../components/ConfirmDeleteDialog";
import { FileEntryRow } from "../../components/FileEntryRow";
import { NameInputDialog } from "../../components/NameInputDialog";
import { Skeleton } from "../../components/Skeleton";
import { useToast } from "../../components/Toast";
import { UploadFileDialog } from "../../components/UploadFileDialog";

interface InlineCrumb {
	label: string;
	path: string;
}

function buildCrumbs(path: string): InlineCrumb[] {
	const crumbs: InlineCrumb[] = [{ label: "data", path: "/" }];
	if (path === "/") return crumbs;
	const segments = path.split("/").filter((s) => s.length > 0);
	for (let i = 0; i < segments.length; i += 1) {
		const accum = `/${segments.slice(0, i + 1).join("/")}`;
		const label = segments[i];
		if (label !== undefined) {
			crumbs.push({ label, path: accum });
		}
	}
	return crumbs;
}

export function FilesBody(): ReactElement {
	const { detail, refresh: refreshDetail } = useServerDetail();
	const router = useRouter();
	const search = useSearchParams();
	const toast = useToast();

	const path = search.get("path") ?? "/";
	const enabled = detail.status === "running" || detail.status === "stopped";

	const { data, status, lastError, refresh } = useFiles(detail.id, path, {
		enabled,
		serverStatus: detail.status,
	});
	const [killing, setKilling] = useState(false);

	const onKillHelper = (): void => {
		setKilling(true);
		killFilesHelper(detail.id)
			.then(() => {
				toast.push("file viewer stopped", "success");
				refreshDetail();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`stop failed · ${msg}`, "error");
			})
			.finally(() => {
				setKilling(false);
			});
	};
	const showKillBar =
		detail.status === "stopped" && detail.files_helper_running;

	const [uploadOpen, setUploadOpen] = useState(false);
	const [folderOpen, setFolderOpen] = useState(false);
	const [renameTarget, setRenameTarget] = useState<FileEntry | null>(null);
	const [confirmFile, setConfirmFile] = useState<FileEntry | null>(null);
	const [confirmDir, setConfirmDir] = useState<FileEntry | null>(null);

	const navigate = (toPath: string): void => {
		const params = new URLSearchParams(Array.from(search.entries()));
		params.set("path", toPath);
		router.push(`?${params.toString()}`);
	};

	const childPath = (name: string): string =>
		path === "/" ? `/${name}` : `${path}/${name}`;

	const triggerDownload = (entry: FileEntry): void => {
		const url = downloadFileUrl(detail.id, childPath(entry.name));
		const a = document.createElement("a");
		a.href = url;
		a.download = entry.name;
		document.body.append(a);
		a.click();
		a.remove();
	};

	// ---------- gates ----------

	if (!enabled) {
		return (
			<Card header="files">
				<p className="px-3 py-2 font-mono text-[12px] text-text-muted">
					server is in transition · refresh in a moment
				</p>
			</Card>
		);
	}

	if (status === "warming") {
		return (
			<Card header="files">
				<p className="mb-3 px-3 py-2 font-mono text-[12px] text-text-muted">
					starting offline file editor…
				</p>
				<Skeleton variant="row" />
				<Skeleton variant="row" />
				<Skeleton variant="row" />
			</Card>
		);
	}

	if (status === "error" && lastError !== null) {
		if (lastError.includes("pvc_not_initialized")) {
			return (
				<Card header="files">
					<div className="flex flex-col gap-3 px-3 py-2">
						<p className="font-mono text-[12px] text-text-muted">
							start the server once to initialize storage.
						</p>
						<div>
							<Button
								variant="primary"
								onClick={() => {
									startServer(detail.id)
										.then(() => {
											toast.push(`${detail.name} · start ok`, "success");
										})
										.catch((err: unknown) => {
											const msg =
												err instanceof ApiError
													? `${err.code}: ${err.message}`
													: err instanceof Error
														? err.message
														: "start failed";
											toast.push(msg, "error");
										});
								}}
							>
								start server
							</Button>
						</div>
					</div>
				</Card>
			);
		}
		return (
			<Card header="files">
				<p className="px-3 py-2 font-mono text-[12px] text-state-error">
					failed to load · {lastError}
				</p>
			</Card>
		);
	}

	if (status === "loading" || data === null) {
		return (
			<Card header="files">
				<Skeleton variant="row" />
				<Skeleton variant="row" />
				<Skeleton variant="row" />
			</Card>
		);
	}

	const crumbs = buildCrumbs(path);

	// ---------- main surface ----------

	return (
		<div className="flex flex-col gap-4">
			{showKillBar && (
				<div className="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border bg-surface px-3 py-2">
					<span className="font-mono text-[12px] text-text-faint">
						file viewer is running · idle
					</span>
					<Button
						variant="danger"
						size="sm"
						onClick={onKillHelper}
						disabled={killing}
					>
						stop file viewer
					</Button>
				</div>
			)}

			<div className="flex flex-wrap items-center justify-between gap-3">
				<nav
					aria-label="path"
					className="flex flex-wrap items-center gap-1 font-mono text-[12px] text-text-muted"
				>
					{crumbs.map((c, i) => (
						<span key={c.path} className="flex items-center gap-1">
							{i > 0 && <span aria-hidden>/</span>}
							{i === crumbs.length - 1 ? (
								<span className="text-text-body">{c.label}</span>
							) : (
								<button
									type="button"
									onClick={() => {
										navigate(c.path);
									}}
									className="rounded-sm hover:text-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
								>
									{c.label}
								</button>
							)}
						</span>
					))}
				</nav>
				<div className="flex gap-2">
					<Button
						onClick={() => {
							setFolderOpen(true);
						}}
					>
						+ folder
					</Button>
					<Button
						variant="primary"
						onClick={() => {
							setUploadOpen(true);
						}}
					>
						upload
					</Button>
				</div>
			</div>

			<Card>
				{data.entries.length === 0 ? (
					<p className="px-3 py-2 font-mono text-[12px] text-text-muted">
						empty directory
					</p>
				) : (
					data.entries.map((entry) => (
						<FileEntryRow
							key={entry.name}
							entry={entry}
							parentPath={path}
							onNavigate={navigate}
							onDownload={() => {
								triggerDownload(entry);
							}}
							onRename={() => {
								setRenameTarget(entry);
							}}
							onDelete={() => {
								if (entry.type === "d") {
									setConfirmDir(entry);
								} else {
									setConfirmFile(entry);
								}
							}}
						/>
					))
				)}
			</Card>

			<UploadFileDialog
				open={uploadOpen}
				onClose={() => {
					setUploadOpen(false);
				}}
				serverId={detail.id}
				parentPath={path}
				onUploaded={refresh}
			/>

			<NameInputDialog
				key="mkdir"
				open={folderOpen}
				onClose={() => {
					setFolderOpen(false);
				}}
				mode="create"
				initialValue=""
				onSubmit={async (name) => {
					await runFileAction(detail.id, {
						action: "mkdir",
						path: childPath(name),
					});
					toast.push(`created ${name}/`, "success");
					refresh();
				}}
			/>

			{renameTarget !== null && (
				<NameInputDialog
					key={`rename-${renameTarget.name}`}
					open={true}
					onClose={() => {
						setRenameTarget(null);
					}}
					mode="rename"
					initialValue={renameTarget.name}
					onSubmit={async (name) => {
						await runFileAction(detail.id, {
							action: "rename",
							from: childPath(renameTarget.name),
							to: childPath(name),
						});
						toast.push(`renamed ${renameTarget.name} → ${name}`, "success");
						refresh();
					}}
				/>
			)}

			{confirmFile !== null && (
				<ConfirmDeleteDialog
					open={true}
					onClose={() => {
						setConfirmFile(null);
					}}
					targetName={confirmFile.name}
					onConfirm={async () => {
						await runFileAction(detail.id, {
							action: "delete",
							path: childPath(confirmFile.name),
							recursive: false,
						});
						toast.push(`deleted ${confirmFile.name}`, "success");
						refresh();
					}}
				/>
			)}

			{confirmDir !== null && (
				<ConfirmDeleteDialog
					open={true}
					onClose={() => {
						setConfirmDir(null);
					}}
					targetName={confirmDir.name}
					busyLabel="deleting recursively…"
					description="this removes the folder and everything inside it. action is irreversible."
					onConfirm={async () => {
						await runFileAction(detail.id, {
							action: "delete",
							path: childPath(confirmDir.name),
							recursive: true,
						});
						toast.push(`deleted ${confirmDir.name}/`, "success");
						refresh();
					}}
				/>
			)}
		</div>
	);
}
