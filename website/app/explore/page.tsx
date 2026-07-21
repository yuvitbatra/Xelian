"use client";

import { useEffect, useState } from "react";
import { browseCatalog, type CatalogPage } from "@/lib/api";
import CatalogCard from "@/components/catalog-card";

type Filter = "all" | "mcp" | "agent";

/**
 * Explore — the discovery index. Hundreds of permissively-licensed MCP servers
 * and AI agents from GitHub, each runnable with one `xelian add` command.
 * Xelian links to and runs them under their own license; it does not host their
 * code (see /ATTRIBUTIONS.md).
 */
export default function Explore() {
  const [page, setPage] = useState<CatalogPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");

  useEffect(() => {
    let cancelled = false;
    const timer = setTimeout(
      () => {
        browseCatalog({ q: query.trim(), type: filter, limit: 90 })
          .then((p) => {
            if (!cancelled) {
              setPage(p);
              setError(null);
            }
          })
          .catch((e: Error) => {
            if (!cancelled) setError(e.message);
          });
      },
      query ? 200 : 0,
    );
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query, filter]);

  const counts = page?.counts;

  return (
    <div className="mx-auto max-w-6xl px-4 sm:px-6">
      <section className="border-b border-gray-200 py-10">
        <h1 className="text-3xl font-semibold tracking-tight text-gray-900">
          Explore the catalog
        </h1>
        <p className="mt-3 max-w-2xl text-base text-gray-600">
          Hundreds of open-source MCP servers and AI agents from GitHub, each
          runnable locally with one command. Xelian imports and runs them under
          their own license — it doesn&apos;t host the code.
        </p>
        {counts ? (
          <p className="mt-2 text-sm text-gray-500">
            {counts.total.toLocaleString()} packages · {counts.mcp} servers ·{" "}
            {counts.agents} agents
          </p>
        ) : null}
      </section>

      <div className="sticky top-0 z-10 flex flex-col gap-3 bg-white/90 py-4 backdrop-blur sm:flex-row sm:items-center">
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search by name, owner, or description…"
          className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:border-gray-400 focus:outline-none"
        />
        <div className="flex shrink-0 gap-1 rounded-lg border border-gray-200 p-1">
          {(["all", "mcp", "agent"] as Filter[]).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`rounded-md px-3 py-1.5 text-sm capitalize ${
                filter === f
                  ? "bg-gray-900 text-white"
                  : "text-gray-600 hover:bg-gray-100"
              }`}
            >
              {f === "mcp" ? "Servers" : f === "agent" ? "Agents" : "All"}
            </button>
          ))}
        </div>
      </div>

      {error ? (
        <div className="rounded-lg border border-amber-200 bg-amber-50 p-4 text-sm text-amber-800">
          Couldn&apos;t reach the registry ({error}). Is it running?
        </div>
      ) : !page ? (
        <p className="py-10 text-sm text-gray-500">Loading…</p>
      ) : page.packages.length === 0 ? (
        <p className="py-10 text-sm text-gray-500">No matches.</p>
      ) : (
        <>
          <div className="grid grid-cols-1 gap-3 pb-10 sm:grid-cols-2 lg:grid-cols-3">
            {page.packages.map((entry) => (
              <CatalogCard key={entry.full_name} entry={entry} />
            ))}
          </div>
          {page.total > page.packages.length ? (
            <p className="pb-10 text-center text-sm text-gray-500">
              Showing {page.packages.length} of {page.total.toLocaleString()} —
              refine your search to see more.
            </p>
          ) : null}
        </>
      )}
    </div>
  );
}
