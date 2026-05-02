import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Static export — see ADR 0003. The Rust backend serves the resulting ./out/.
  output: "export",
  // No Node-side image optimization in static export.
  images: { unoptimized: true },
  // Explicitly opt out of trailing slashes so /api/health stays /api/health
  // (matches the backend route exactly, no redirect dance).
  trailingSlash: false,
  // NOTE: rewrites() are unsupported with output: 'export' in Next.js 16. The
  // dev workflow for M1 is single-binary local: `pnpm build` once, then
  // `cd ../backend && cargo run --features serve-dir`.
};

export default nextConfig;
