"use client";

import { useEffect, useState, type ReactElement } from "react";

import { getMe, logout, type Me } from "../lib/api";

import { IconButton } from "./IconButton";
import { PathBreadcrumb } from "./PathBreadcrumb";
import { Skeleton } from "./Skeleton";

type State = { kind: "loading" } | { kind: "ok"; me: Me } | { kind: "error" };

export function CommandBar(): ReactElement {
	const [state, setState] = useState<State>({ kind: "loading" });

	useEffect(() => {
		let alive = true;
		getMe()
			.then((me) => {
				if (alive) setState({ kind: "ok", me });
			})
			.catch(() => {
				if (alive) setState({ kind: "error" });
			});
		return () => {
			alive = false;
		};
	}, []);

	const onLogout = (): void => {
		logout()
			.then((url) => {
				window.location.href = url;
			})
			.catch(() => {
				// Best-effort — if the logout endpoint fails, fall back to
				// reloading so the session cookie still gets cleared on next nav.
				window.location.reload();
			});
	};

	return (
		<header className="flex h-12 items-center justify-between border-b border-border-soft bg-bg px-5">
			<PathBreadcrumb />
			<div className="flex items-center gap-3">
				{state.kind === "loading" && (
					<Skeleton variant="text" className="h-3 w-32" />
				)}
				{state.kind === "ok" && (
					<>
						{state.me.picture !== null && state.me.picture !== undefined ? (
							// eslint-disable-next-line @next/next/no-img-element
							<img
								src={state.me.picture}
								alt=""
								className="h-6 w-6 rounded-full"
							/>
						) : null}
						<span className="font-mono text-[12px] text-text-body">
							{state.me.name}
						</span>
						<IconButton aria-label="logout" onClick={onLogout}>
							<svg
								viewBox="0 0 24 24"
								width="16"
								height="16"
								fill="none"
								stroke="currentColor"
								strokeWidth={2}
							>
								<path d="M16 17l5-5-5-5M21 12H9M9 21H5a2 2 0 01-2-2V5a2 2 0 012-2h4" />
							</svg>
						</IconButton>
					</>
				)}
				{state.kind === "error" && (
					<span className="font-mono text-[12px] text-state-error">
						auth error
					</span>
				)}
			</div>
		</header>
	);
}
