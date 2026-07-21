"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { browseCatalog } from "@/lib/api";

/**
 * Hero call-to-action showing how many packages are browsable in the discovery
 * catalog, linking to /explore. Makes the landing page convey a rich directory
 * (hundreds of runnable repos) rather than only the handful published as
 * archives.
 */
export default function CatalogTeaser() {
  const [counts, setCounts] = useState<{
    total: number;
    mcp: number;
    agents: number;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    browseCatalog({ limit: 1 })
      .then((p) => {
        if (!cancelled) setCounts(p.counts);
      })
      .catch(() => {
        /* registry unreachable — hide the teaser rather than error the hero */
      });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!counts || counts.total === 0) return null;

  return (
    <div className="mx-auto mt-8 max-w-xl">
      <Link
        href="/explore"
        className="group flex items-center justify-center gap-2 text-sm text-gray-600 hover:text-gray-900"
      >
        <span className="font-medium text-gray-900">
          Explore {counts.total.toLocaleString()} packages
        </span>
        <span className="text-gray-400">
          · {counts.mcp} MCP servers · {counts.agents} agents · run any with one
          command
        </span>
        <span aria-hidden className="transition-transform group-hover:translate-x-0.5">
          →
        </span>
      </Link>
    </div>
  );
}
