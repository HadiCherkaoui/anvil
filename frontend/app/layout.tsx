import type { Metadata } from "next";
import { Fira_Code, Fira_Sans } from "next/font/google";
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
  // Background per spec §1.5: slate-900 (#0F172A) — the chart's "Dark Mode
  // (OLED)" decision codified the slate palette but settled on -900 (not the
  // even-darker -950) for the surface color.
  const bodyClass = [
    firaSans.variable,
    firaCode.variable,
    "min-h-screen",
    "bg-slate-900",
    "text-slate-100",
    "font-sans",
    "antialiased",
  ].join(" ");
  return (
    <html lang="en" className="dark">
      <body className={bodyClass}>{children}</body>
    </html>
  );
}
