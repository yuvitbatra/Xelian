"use client";

import { useEffect, useMemo, useState } from "react";
import { browseCatalog, type CatalogEntry } from "@/lib/api";
import CatalogCard from "@/components/catalog-card";
import CopyCommand from "@/components/copy-command";

type Filter = "all" | "agent" | "mcp";

const PAGE = 60;

export default function Home() {
  const [entries, setEntries] = useState<CatalogEntry[] | null>(null);
  const [total, setTotal] = useState(0);
  const [counts, setCounts] = useState<{ mcp: number; agents: number } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [shown, setShown] = useState(PAGE);

  // Reset how many are shown whenever the query/filter changes.
  useEffect(() => setShown(PAGE), [query, filter]);

  useEffect(() => {
    const q = query.trim();
    let cancelled = false;
    const timer = setTimeout(
      () => {
        browseCatalog({ q, type: filter, limit: 300 })
          .then((p) => {
            if (!cancelled) {
              setEntries(p.packages);
              setTotal(p.total);
              setCounts(p.counts);
              setError(null);
            }
          })
          .catch((e: Error) => !cancelled && setError(e.message));
      },
      q ? 200 : 0,
    );
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [query, filter]);

  const visible = useMemo(() => (entries ?? []).slice(0, shown), [entries, shown]);

  return (
    <div className="mx-auto max-w-6xl px-4 sm:px-6">
      <section className="border-b border-gray-200 py-12 text-center">
        <h1 className="mx-auto max-w-2xl text-4xl font-semibold tracking-tight text-gray-900">
          Run AI agents like you run models
        </h1>
        <p className="mx-auto mt-4 max-w-xl text-base text-gray-600">
          A registry of runnable AI agents and MCP servers. Install once, then
          run any of them locally with a single command.
        </p>
        <div className="mx-auto mt-6 max-w-md">
          <CopyCommand command="xelian run owner/name" />
        </div>
        {counts ? (
          <p className="mt-4 text-sm text-gray-500">
            {(counts.mcp + counts.agents).toLocaleString()} packages ·{" "}
            {counts.mcp} MCP servers · {counts.agents} agents
          </p>
        ) : null}
      </section>

      {/* One-line disclaimer for the catalog (third-party, under their licenses). */}
      <p className="mt-6 rounded-lg border border-gray-200 bg-gray-50 px-4 py-2.5 text-center text-sm text-gray-600">
        These are third-party open-source projects, each run under its own
        license — Xelian links to the source and runs it, it doesn&apos;t host
        the code.
      </p>

      <div className="sticky top-0 z-10 mt-4 flex flex-col gap-3 bg-white/90 py-4 backdrop-blur sm:flex-row sm:items-center">
        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search agents and MCP servers…"
          className="w-full rounded-lg border border-gray-300 px-3 py-2 text-sm focus:border-gray-400 focus:outline-none"
        />
        <div className="flex shrink-0 gap-1 rounded-lg border border-gray-200 p-1">
          {(["all", "mcp", "agent"] as Filter[]).map((f) => (
            <button
              key={f}
              onClick={() => setFilter(f)}
              className={`rounded-md px-3 py-1.5 text-sm ${
                filter === f
                  ? "bg-gray-900 text-white"
                  : "text-gray-600 hover:bg-gray-100"
              }`}
            >
              {f === "mcp" ? "MCP servers" : f === "agent" ? "Agents" : "All"}
            </button>
          ))}
        </div>
      </div>

      {error ? (
        <div className="rounded-lg border border-amber-200 bg-amber-50 p-4 text-sm text-amber-800">
          Couldn&apos;t reach the registry ({error}).
        </div>
      ) : !entries ? (
        <p className="py-12 text-center text-sm text-gray-500">Loading…</p>
      ) : visible.length === 0 ? (
        <p className="py-12 text-center text-sm text-gray-500">No matches.</p>
      ) : (
        <>
          <div className="grid grid-cols-1 gap-3 pb-6 sm:grid-cols-2 lg:grid-cols-3">
            {visible.map((entry) => (
              <CatalogCard key={entry.full_name} entry={entry} />
            ))}
          </div>
          {shown < (entries?.length ?? 0) ? (
            <div className="pb-12 text-center">
              <button
                onClick={() => setShown((s) => s + PAGE)}
                className="rounded-lg border border-gray-300 px-5 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50"
              >
                Show more ({total - shown} more)
              </button>
            </div>
          ) : (
            <p className="pb-12 text-center text-xs text-gray-400">
              Showing all {visible.length} matches.
            </p>
          )}
        </>
      )}
    </div>
  );
}
