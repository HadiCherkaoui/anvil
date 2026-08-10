// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Metadata } from "next";
import { Fira_Code, Fira_Sans } from "next/font/google";
import { Colophon } from "./components/Colophon";
import { CommandBar } from "./components/CommandBar";
import { ToastProvider } from "./components/Toast";
import "./globals.css";

const firaSans = Fira_Sans({
	subsets: ["latin"],
	weight: ["400", "500", "600"],
	variable: "--font-fira-sans",
	display: "swap",
});

const firaCode = Fira_Code({
	subsets: ["latin"],
	weight: ["400", "500"],
	variable: "--font-fira-code",
	display: "swap",
});

export const metadata: Metadata = {
	title: "Anvil",
	description: "k8s-native Minecraft server panel",
};

export default function RootLayout({
	children,
}: Readonly<{ children: React.ReactNode }>): React.ReactElement {
	const bodyClass = [
		firaSans.variable,
		firaCode.variable,
		"min-h-screen",
		// Column so the colophon rail sits on the viewport floor on short pages
		// instead of floating directly under the content.
		"flex",
		"flex-col",
		"font-sans",
		"antialiased",
	].join(" ");
	return (
		<html lang="en" className="dark">
			<body className={bodyClass}>
				<ToastProvider>
					<CommandBar />
					<main className="flex-1">{children}</main>
					<Colophon />
				</ToastProvider>
			</body>
		</html>
	);
}
