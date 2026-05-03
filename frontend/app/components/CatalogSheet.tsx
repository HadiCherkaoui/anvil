"use client";

import { useEffect, useState, type ReactElement } from "react";

import {
	ApiError,
	fetchCatalogVersions,
	searchCatalog,
	type CatalogHit,
	type CatalogVersion,
} from "../lib/api";

import { Button } from "./Button";
import { Sheet } from "./Sheet";
import { Skeleton } from "./Skeleton";

type Mode = "modpack" | "mod";
type Loader = "fabric" | "forge" | "neoforge";

export interface CatalogPick {
	hit: CatalogHit;
	version: CatalogVersion;
}

interface Props {
	isOpen: boolean;
	onClose: () => void;
	mode: Mode;
	loader?: Loader;
	mc?: string;
	onPick: (pick: CatalogPick) => void;
}

const SEARCH_DEBOUNCE_MS = 300;

export function CatalogSheet({
	isOpen,
	onClose,
	mode,
	loader,
	mc,
	onPick,
}: Props): ReactElement {
	const [q, setQ] = useState("");
	const [hits, setHits] = useState<readonly CatalogHit[]>([]);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [activeHit, setActiveHit] = useState<CatalogHit | null>(null);
	const [versions, setVersions] = useState<readonly CatalogVersion[]>([]);

	const handleClose = (): void => {
		setQ("");
		setHits([]);
		setError(null);
		setActiveHit(null);
		setVersions([]);
		onClose();
	};

	useEffect(() => {
		if (!isOpen || q.trim().length === 0) return undefined;
		const ctrl = new AbortController();
		const t = window.setTimeout(() => {
			setBusy(true);
			setError(null);
			const params: Parameters<typeof searchCatalog>[0] = {
				type: mode,
				q: q.trim(),
			};
			if (mode === "mod" && loader !== undefined) params.loader = loader;
			if (mode === "mod" && mc !== undefined) params.mc = mc;
			searchCatalog(params, ctrl.signal)
				.then((r) => {
					setHits(r);
				})
				.catch((err: unknown) => {
					if (err instanceof DOMException && err.name === "AbortError") return;
					setError(
						err instanceof ApiError
							? `${err.code}: ${err.message}`
							: err instanceof Error
								? err.message
								: "search failed",
					);
				})
				.finally(() => {
					setBusy(false);
				});
		}, SEARCH_DEBOUNCE_MS);
		return () => {
			ctrl.abort();
			window.clearTimeout(t);
		};
	}, [isOpen, q, mode, loader, mc]);

	const onPickHit = (hit: CatalogHit): void => {
		setActiveHit(hit);
		setVersions([]);
		const ctrl = new AbortController();
		const opts: { loader?: string; mc?: string } = {};
		if (loader !== undefined) opts.loader = loader;
		if (mc !== undefined) opts.mc = mc;
		fetchCatalogVersions(hit.provider, hit.project_id, opts, ctrl.signal)
			.then(setVersions)
			.catch((err: unknown) => {
				setError(
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "version fetch failed",
				);
			});
	};

	return (
		<Sheet
			isOpen={isOpen}
			onClose={handleClose}
			title={mode === "mod" ? "browse mods" : "browse modpacks"}
			width={720}
		>
			<div className="flex h-full flex-col">
				<div className="border-b border-border-soft px-5 py-3">
					<input
						value={q}
						onChange={(e) => {
							setQ(e.target.value);
							setActiveHit(null);
						}}
						placeholder={mode === "mod" ? "search mods" : "search modpacks"}
						className="w-full rounded-md border border-border bg-bg px-3 py-2 font-mono text-[13px] text-text-body placeholder:text-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						spellCheck={false}
					/>
					{mode === "mod" && (
						<p className="mt-2 font-mono text-[11px] text-text-faint">
							filtered to {loader ?? "any loader"} ·{" "}
							{mc ?? "any minecraft version"}
						</p>
					)}
				</div>

				{activeHit !== null ? (
					<div className="flex-1 overflow-y-auto px-5 py-3">
						<button
							type="button"
							onClick={() => {
								setActiveHit(null);
							}}
							className="mb-3 font-mono text-[11px] text-text-muted hover:text-text-body"
						>
							← back to results
						</button>
						<h3 className="font-mono text-[14px] text-text-primary">
							{activeHit.name}
						</h3>
						<p className="mt-1 font-mono text-[12px] text-text-muted">
							pick a version
						</p>
						<ul className="mt-3 flex flex-col gap-1">
							{versions.length === 0 && (
								<li className="font-mono text-[12px] text-text-faint">
									no compatible versions
								</li>
							)}
							{versions.map((v) => (
								<li
									key={v.version_id}
									className="flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
								>
									<div>
										<span className="text-text-body">{v.version_name}</span>
										<span className="ml-2 text-text-faint">{v.channel}</span>
									</div>
									<Button
										variant="primary"
										onClick={() => {
											onPick({ hit: activeHit, version: v });
											handleClose();
										}}
									>
										install
									</Button>
								</li>
							))}
						</ul>
					</div>
				) : (
					<div className="flex-1 overflow-y-auto">
						{busy &&
							Array.from({ length: 4 }).map((_, i) => (
								<Skeleton key={i} variant="row" className="mx-5 my-2 h-12" />
							))}
						{!busy && error !== null && (
							<p className="px-5 py-3 font-mono text-[12px] text-state-error">
								{error}
							</p>
						)}
						{!busy &&
							error === null &&
							q.trim().length > 0 &&
							hits.length === 0 && (
								<p className="px-5 py-3 font-mono text-[12px] text-text-faint">
									no results
								</p>
							)}
						{!busy && q.trim().length === 0 && (
							<p className="px-5 py-3 font-mono text-[12px] text-text-faint">
								start typing to search
							</p>
						)}
						<ul>
							{hits.map((h) => (
								<li
									key={`${h.provider}:${h.project_id}`}
									className="group flex items-center gap-3 border-b border-border-soft px-5 py-2"
								>
									<span
										className="h-3.5 w-1 rounded-sm"
										style={{
											background:
												h.provider === "modrinth"
													? "var(--color-source-modrinth)"
													: "var(--color-source-curseforge)",
										}}
									/>
									<div className="flex-1">
										<p className="font-mono text-[13px] text-text-body">
											{h.name}
										</p>
										<p className="font-mono text-[11px] text-text-muted">
											{h.author ?? ""} · {h.downloads.toLocaleString()}{" "}
											downloads
										</p>
									</div>
									<Button
										onClick={() => {
											onPickHit(h);
										}}
									>
										pick
									</Button>
								</li>
							))}
						</ul>
					</div>
				)}
			</div>
		</Sheet>
	);
}
