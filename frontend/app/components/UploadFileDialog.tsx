"use client";

import { useRef, useState, type ReactElement } from "react";

import { ApiError, uploadFile } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

const UPLOAD_CAP_BYTES = 100 * 1024 * 1024;

export interface UploadFileDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
	/** Directory path to upload into, e.g. "/mods" or "/". */
	parentPath: string;
	onUploaded: () => void;
}

export function UploadFileDialog({
	open,
	onClose,
	serverId,
	parentPath,
	onUploaded,
}: UploadFileDialogProps): ReactElement {
	const [file, setFile] = useState<File | null>(null);
	const [progress, setProgress] = useState(0);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const abortRef = useRef<AbortController | null>(null);
	const toast = useToast();
	const noop = (): void => undefined;

	const reset = (): void => {
		setFile(null);
		setProgress(0);
		setBusy(false);
		setError(null);
		abortRef.current = null;
	};

	const handleClose = (): void => {
		abortRef.current?.abort();
		reset();
		onClose();
	};

	const targetPath = (): string => {
		if (file === null) return parentPath;
		const base = parentPath.endsWith("/") ? parentPath : `${parentPath}/`;
		return `${base}${file.name}`;
	};

	const send = (): void => {
		if (file === null) return;
		// Reject path-traversal characters and dotfiles outright. Stripping
		// would silently rename the user's file; rejection is honest.
		if (
			file.name.includes("/") ||
			file.name.includes("\\") ||
			file.name.includes("\0") ||
			file.name.startsWith(".")
		) {
			const message =
				"invalid filename · cannot contain '/', '\\', NUL, or start with '.'";
			setError(message);
			toast.push(message, "error");
			return;
		}
		if (file.size > UPLOAD_CAP_BYTES) {
			setError(
				`file too large (max ${(UPLOAD_CAP_BYTES / 1024 / 1024).toString()} MiB)`,
			);
			return;
		}
		setBusy(true);
		setError(null);
		setProgress(0);
		const ctrl = new AbortController();
		abortRef.current = ctrl;
		uploadFile(serverId, targetPath(), file, {
			onProgress: setProgress,
			signal: ctrl.signal,
		})
			.then(() => {
				toast.push(`uploaded ${file.name}`, "success");
				onUploaded();
				reset();
				onClose();
			})
			.catch((err: unknown) => {
				const message =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "upload failed";
				setError(message);
				setBusy(false);
			});
	};

	return (
		<Modal open={open} onClose={busy ? noop : handleClose} title="upload file">
			<div className="flex flex-col gap-3 font-mono text-[13px]">
				<p className="text-text-muted">
					uploading into <span className="text-text-body">{parentPath}</span>
				</p>
				<input
					type="file"
					onChange={(e) => {
						setFile(e.target.files?.[0] ?? null);
						setError(null);
					}}
					disabled={busy}
					className="block w-full text-text-body"
				/>
				{file !== null && (
					<p className="text-text-muted">
						{file.name} · {Math.round(file.size / 1024).toString()} KiB
					</p>
				)}
				{busy && (
					<div className="h-1 w-full overflow-hidden rounded-sm bg-border">
						<div
							className="h-full bg-accent transition-[width] duration-150"
							style={{ width: `${Math.round(progress * 100).toString()}%` }}
						/>
					</div>
				)}
				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={handleClose}>{busy ? "cancel" : "close"}</Button>
					<Button
						variant="primary"
						onClick={send}
						disabled={file === null || busy}
					>
						{busy ? "uploading…" : "send"}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
