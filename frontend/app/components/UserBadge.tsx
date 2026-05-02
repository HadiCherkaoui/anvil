"use client";

import { useEffect, useState } from "react";
import { Button } from "./Button";
import { getMe, logout, type Me } from "../lib/api";

export function UserBadge(): React.ReactElement | null {
	const [me, setMe] = useState<Me | null>(null);

	useEffect(() => {
		const ctrl = new AbortController();
		getMe(ctrl.signal)
			.then(setMe)
			.catch(() => undefined);
		return () => {
			ctrl.abort();
		};
	}, []);

	if (me === null) return null;

	return (
		<div className="fixed top-3 right-4 z-50 flex items-center gap-3 rounded-full border border-slate-800 bg-slate-900/80 px-3 py-1.5 backdrop-blur">
			{me.picture !== null ? (
				// Authentik serves arbitrary avatar URLs; next/image needs allow-list
				// configuration we don't want to maintain. Static-export rendering is
				// fine with a plain <img>.
				// eslint-disable-next-line @next/next/no-img-element
				<img
					src={me.picture}
					alt=""
					width={28}
					height={28}
					className="size-7 rounded-full"
				/>
			) : (
				<div className="size-7 rounded-full bg-slate-700" />
			)}
			<span className="text-xs text-slate-200">{me.name}</span>
			<Button
				variant="secondary"
				className="px-2 py-1 text-xs"
				onClick={() => {
					logout()
						.then((url) => {
							window.location.href = url;
						})
						.catch(() => undefined);
				}}
			>
				sign out
			</Button>
		</div>
	);
}
