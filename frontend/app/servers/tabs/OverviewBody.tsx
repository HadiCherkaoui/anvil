"use client";

import type { ReactElement } from "react";

import { Card } from "../../components/Card";
import { useServerDetailCtx } from "../../lib/server-detail-context";

export function OverviewBody(): ReactElement {
	const detail = useServerDetailCtx();
	return (
		<div className="grid gap-4 lg:grid-cols-2">
			<Card header="connection">
				<pre className="font-mono text-[12px] text-text-body">
					{detail.endpoint
						? `${detail.endpoint.host}:${detail.endpoint.port.toString()}`
						: "address pending…"}
				</pre>
				<dl className="mt-4 grid grid-cols-[8rem_1fr] gap-y-1 font-mono text-[11px]">
					<dt className="text-text-muted">exposure</dt>
					<dd className="text-text-body">{detail.exposure_mode}</dd>
					{detail.nodeport !== null && (
						<>
							<dt className="text-text-muted">nodeport</dt>
							<dd className="text-text-body">{detail.nodeport}</dd>
						</>
					)}
					{detail.last_started_at !== null && (
						<>
							<dt className="text-text-muted">last started</dt>
							<dd className="text-text-body">
								{new Date(detail.last_started_at * 1000).toLocaleString()}
							</dd>
						</>
					)}
				</dl>
			</Card>

			<Card header="at a glance">
				<dl className="grid grid-cols-[8rem_1fr] gap-y-1 font-mono text-[12px]">
					<dt className="text-text-muted">runtime</dt>
					<dd className="text-text-body">{detail.server_type}</dd>
					<dt className="text-text-muted">mc version</dt>
					<dd className="text-text-body">{detail.mc_version}</dd>
					<dt className="text-text-muted">cpu limit</dt>
					<dd className="text-text-body">
						{(detail.cpu_millicores / 1000).toFixed(2)} cores
					</dd>
					<dt className="text-text-muted">memory limit</dt>
					<dd className="text-text-body">{detail.memory_mi} MiB</dd>
					<dt className="text-text-muted">storage</dt>
					<dd className="text-text-body">
						{detail.storage_size_gi} GiB
						{detail.storage_class !== null && (
							<span className="ml-1 text-text-muted">
								· {detail.storage_class}
							</span>
						)}
					</dd>
				</dl>
			</Card>
		</div>
	);
}
