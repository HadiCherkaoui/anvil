// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Suspense, type ReactElement } from "react";

import { ServerDetailView } from "./ServerDetailView";

export default function ServerDetailPage(): ReactElement {
	// useSearchParams forces a CSR bail-out under static export, so the
	// route must be wrapped in a Suspense boundary at the page level.
	return (
		<Suspense fallback={null}>
			<ServerDetailView />
		</Suspense>
	);
}
