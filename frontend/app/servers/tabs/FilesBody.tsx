"use client";

import type { ReactElement } from "react";

import { Card } from "../../components/Card";

export function FilesBody(): ReactElement {
	return (
		<Card header="files">
			<p className="font-mono text-[12px] text-text-muted">
				in-app file browser arrives in v2.3. for now, use{" "}
				<a
					href="https://files.cherkaoui.ch"
					className="rounded-sm text-accent hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
					target="_blank"
					rel="noreferrer"
				>
					files.cherkaoui.ch
				</a>
				.
			</p>
		</Card>
	);
}
