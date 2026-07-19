"use client";

import { useEffect, useMemo, useState } from "react";
import {
  listPackages,
  searchPackages,
  type PackageSummary,
} from "@/lib/api";
import PackageCard from "@/components/package-card";
import CopyCommand from "@/components/copy-command";

type Filter = "all" | "agent" | "mcp";

export default function Home() {
  const [packages, setPackages] = useState<PackageSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");

  // Search goes through the registry's /search endpoint (H-224) — the same
  // surface `xelian search` uses — debounced; empty query lists everything.
  useEffect(() => {
    const q = query.trim();
    let cancelled = false;
    const timer = setTimeout(
      () => {
        (q ? searchPackages(q) : listPackages())
          .then((rows) => {
            if (!cancelled) {
              setPackages(rows);
              setError(null);
            }
          })
          .catch((e: Error) => {
            if (!cancelled) setError(e.message);
          });
      },
      q ? 200 : 0,
    );
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query]);

  const visible = useMemo(() => {
    if (!packages) return [];
    return packages.filter(
      (p) => filter === "all" || p.package_type === filter,
    );
  }, [packages, filter]);

  return (
    <div className="mx-auto max-w-6xl px-4 sm:px-6">
      <section className="border-b border-gray-200 py-14 text-center">
        <h1 className="mx-auto max-w-2xl text-4xl font-semibold tracking-tight text-gray-900">
          Run AI agents like you run models
        </h1>
        <p className="mx-auto mt-4 max-w-xl text-base text-gray-600">
          Xelian is a local-first registry and runtime for AI agents and MCP
          servers. One command downloads, installs, and launches any package.
        </p>
        <div className="mx-auto mt-6 max-w-md">
          <CopyCommand command="xelian run username/my-agent" />
        </div>
      </section>

      <section className="py-8">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex items-center gap-2">
            <h2 className="text-lg font-semibold text-gray-900">Packages</h2>
            {packages ? (
              <span className="text-sm text-gray-500">{visible.length}</span>
            ) : null}
          </div>
          <div className="flex items-center gap-2">
            <div className="flex rounded-md border border-gray-300 p-0.5">
              {(["all", "agent", "mcp"] as const).map((f) => (
                <button
                  key={f}
                  onClick={() => setFilter(f)}
                  className={`rounded px-2.5 py-1 text-xs font-medium ${
                    filter === f
                      ? "bg-gray-900 text-white"
                      : "text-gray-600 hover:text-gray-900"
                  }`}
                >
                  {f === "all" ? "All" : f === "agent" ? "Agents" : "MCP servers"}
                </button>
              ))}
            </div>
            <input
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search packages"
              className="w-full rounded-md border border-gray-300 px-3 py-1.5 text-sm text-gray-900 placeholder:text-gray-400 focus:border-gray-500 focus:outline-none sm:w-64"
            />
          </div>
        </div>

        <div className="mt-6">
          {error ? (
            <div className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700">
              Could not reach the registry: {error}
            </div>
          ) : packages === null ? (
            <p className="py-12 text-center text-sm text-gray-500">
              Loading packages…
            </p>
          ) : visible.length === 0 ? (
            <p className="py-12 text-center text-sm text-gray-500">
              {query.trim() || packages.length > 0
                ? "No packages match your search."
                : "No packages published yet. Publish one from the CLI with xelian push, or on this site."}
            </p>
          ) : (
            <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3">
              {visible.map((p) => (
                <PackageCard key={`${p.owner}/${p.name}`} pkg={p} />
              ))}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
