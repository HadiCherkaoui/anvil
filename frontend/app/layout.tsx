import type { Metadata } from "next";
import { Fira_Code, Fira_Sans } from "next/font/google";
import { CommandBar } from "./components/CommandBar";
import { ToastProvider } from "./components/Toast";
import "./globals.css";

// next/font self-hosts the Google Fonts at build time, so the static export
// has no external network dependency at request time.
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
		"font-sans",
		"antialiased",
	].join(" ");
	return (
		<html lang="en" className="dark">
			<body className={bodyClass}>
				<ToastProvider>
					<CommandBar />
					<main>{children}</main>
				</ToastProvider>
			</body>
		</html>
	);
}
