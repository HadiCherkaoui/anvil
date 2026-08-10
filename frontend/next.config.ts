// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Static export — see ADR 0003. The Rust backend serves the resulting ./out/.
  output: "export",
  // No Node-side image optimization in static export.
  images: { unoptimized: true },
  // Explicitly opt out of trailing slashes so /api/health stays /api/health
  // (matches the backend route exactly, no redirect dance).
  trailingSlash: false,
};

export default nextConfig;
