"use client";

import { useState } from "react";
import type { CatalogEntry } from "@/lib/api";
import TypeBadge from "./type-badge";

/** A single discovery-index entry: a real GitHub repo runnable via `xelian add`. */
export default function CatalogCard({ entry }: { entry: CatalogEntry }) {
  const [copied, setCopied] = useState(false);
  // Prefer the short `run owner/repo` form — it resolves through the catalog to
  // the same GitHub source. Falls back to the explicit URL if full_name is absent.
  const command = entry.full_name
    ? `xelian run ${entry.full_name}`
    : `xelian add ${entry.url}`;

  return (
    <div className="flex flex-col rounded-lg border border-gray-200 bg-white p-4 transition-colors hover:border-gray-300">
      <div className="flex items-start justify-between gap-2">
        <a
          href={entry.url}
          target="_blank"
          rel="noreferrer"
          className="truncate font-mono text-sm font-medium text-gray-900 hover:underline"
        >
          {entry.full_name}
        </a>
        <TypeBadge type={entry.type} />
      </div>

      {entry.description ? (
        <p className="mt-2 line-clamp-2 text-sm text-gray-600">
          {entry.description}
        </p>
      ) : null}

      <div className="mt-3 flex items-center gap-3 text-xs text-gray-500">
        <span>★ {entry.stars.toLocaleString()}</span>
        {entry.license ? <span>{entry.license}</span> : null}
      </div>

      <button
        onClick={async () => {
          await navigator.clipboard.writeText(command);
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        }}
        className="mt-3 flex items-center justify-between gap-2 rounded-md border border-gray-200 bg-gray-50 px-3 py-2 text-left hover:bg-gray-100"
        title="Copy the command to run this locally"
      >
        <code className="truncate font-mono text-xs text-gray-800">
          {command}
        </code>
        <span className="shrink-0 text-xs text-gray-500">
          {copied ? "Copied" : "Copy"}
        </span>
      </button>
    </div>
  );
}
