import type { NextConfig } from "next";

const nextConfig: NextConfig = {
	// Static export — see ADR 0003. The Rust backend serves the resulting ./out/.
	output: "export",
	// No Node-side image optimization in static export.
	images: { unoptimized: true },
};

export default nextConfig;
