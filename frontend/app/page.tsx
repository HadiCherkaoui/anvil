"use client";

import { useEffect, useState } from "react";

import { fetchServers, type ServerSummary } from "./lib/api";

type LoadState =
  | { readonly kind: "loading" }
  | { readonly kind: "ready"; readonly servers: readonly ServerSummary[] }
  | { readonly kind: "error"; readonly message: string };

export default function HomePage(): React.ReactElement {
  const [state, setState] = useState<LoadState>({ kind: "loading" });

  useEffect(() => {
    const controller = new AbortController();
    fetchServers(controller.signal)
      .then((servers) => {
        setState({ kind: "ready", servers });
      })
      .catch((err: unknown) => {
        if (err instanceof DOMException && err.name === "AbortError") return;
        const message = err instanceof Error ? err.message : "unknown error";
        setState({ kind: "error", message });
      });
    return () => {
      controller.abort();
    };
  }, []);

  return (
    <main className="min-h-screen px-6 py-12">
      <header className="mx-auto mb-12 flex max-w-5xl items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">anvil</h1>
        <span className="font-mono text-xs text-slate-400">M1 walking skeleton</span>
      </header>
      <section className="mx-auto max-w-5xl">
        {state.kind === "loading" && (
          <p className="text-sm text-slate-400">loading servers…</p>
        )}
        {state.kind === "error" && (
          <p className="text-sm text-red-400">failed to load servers: {state.message}</p>
        )}
        {state.kind === "ready" && state.servers.length === 0 && <EmptyState />}
        {state.kind === "ready" && state.servers.length > 0 && (
          <p className="text-sm text-slate-400">
            {state.servers.length.toString()} server(s) — full table lands in M3.
          </p>
        )}
      </section>
    </main>
  );
}

function EmptyState(): React.ReactElement {
  return (
    <div className="flex flex-col items-center gap-4 py-24 text-center">
      <ServerIcon />
      <h2 className="text-lg font-medium">No servers yet.</h2>
      <p className="max-w-sm text-sm text-slate-400">
        Managed Minecraft servers will appear here once you create them.
      </p>
      <button
        type="button"
        // Spec §1.1 mandates the new-server button on the empty state.
        // The handler ships in M2 — keeping the button visible and explicit
        // about its M2-status communicates the seam to a casual visitor.
        disabled
        title="Available in M2 (POST /api/servers)"
        className="mt-2 inline-flex cursor-not-allowed items-center gap-2 rounded-md bg-green-500/20 px-3 py-1.5 text-sm font-medium text-green-300 opacity-60"
      >
        + new server
      </button>
    </div>
  );
}

function ServerIcon(): React.ReactElement {
  // Lucide-style line SVG, 24×24 viewBox, 2-stroke (per spec §1.5).
  return (
    <svg
      aria-hidden
      className="text-slate-600"
      width={48}
      height={48}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <rect x="2" y="2" width="20" height="8" rx="2" />
      <rect x="2" y="14" width="20" height="8" rx="2" />
      <line x1="6" y1="6" x2="6.01" y2="6" />
      <line x1="6" y1="18" x2="6.01" y2="18" />
    </svg>
  );
}
