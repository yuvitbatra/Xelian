"use client";

import { useEffect, useState } from "react";
import { useParams } from "next/navigation";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeSanitize from "rehype-sanitize";
import { browseCatalog, type CatalogEntry } from "@/lib/api";
import CopyCommand from "@/components/copy-command";
import TypeBadge from "@/components/type-badge";

/**
 * Model card for a catalog package — HuggingFace-style: the project's README,
 * its metadata, and the one command to run it. Data is the catalog entry (from
 * the registry) plus the README fetched directly from GitHub.
 */
export default function CatalogCardPage() {
  const params = useParams<{ owner: string; name: string }>();
  const owner = params?.owner ?? "";
  const name = params?.name ?? "";

  const [entry, setEntry] = useState<CatalogEntry | null>(null);
  const [readme, setReadme] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!owner || !name) return;
    // Resolve the catalog entry by searching for its full name.
    browseCatalog({ q: name, limit: 50 })
      .then((p) => {
        const hit =
          p.packages.find(
            (e) => e.full_name.toLowerCase() === `${owner}/${name}`.toLowerCase(),
          ) ?? null;
        setEntry(hit);
        if (!hit) setError("Not found in the catalog.");
      })
      .catch((e: Error) => setError(e.message));
  }, [owner, name]);

  useEffect(() => {
    if (!owner || !name) return;
    // Fetch the README from GitHub (main, then master).
    const tryBranch = async (branch: string) => {
      const r = await fetch(
        `https://raw.githubusercontent.com/${owner}/${name}/${branch}/README.md`,
      );
      return r.ok ? r.text() : null;
    };
    (async () => {
      const md = (await tryBranch("main")) ?? (await tryBranch("master"));
      setReadme(md);
    })().catch(() => setReadme(null));
  }, [owner, name]);

  if (error && !entry) {
    return (
      <div className="mx-auto max-w-4xl px-4 py-16 sm:px-6">
        <p className="rounded-lg border border-amber-200 bg-amber-50 p-4 text-sm text-amber-800">
          {error}
        </p>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-4xl px-4 py-8 sm:px-6">
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="font-mono text-xl font-semibold text-gray-900">
          {owner}/{name}
        </h1>
        {entry ? <TypeBadge type={entry.type} /> : null}
      </div>

      {entry ? (
        <>
          {entry.description ? (
            <p className="mt-2 text-gray-600">{entry.description}</p>
          ) : null}
          <div className="mt-3 flex flex-wrap items-center gap-4 text-sm text-gray-500">
            <span>★ {entry.stars.toLocaleString()}</span>
            {entry.license ? <span>{entry.license}</span> : null}
            <a
              href={entry.url}
              target="_blank"
              rel="noreferrer"
              className="text-gray-700 underline hover:text-gray-900"
            >
              View on GitHub →
            </a>
          </div>
          <div className="mt-4 max-w-lg">
            <CopyCommand command={`xelian run ${entry.full_name}`} />
          </div>
          <p className="mt-2 text-xs text-gray-400">
            Third-party project — run under its own license ({entry.license}).
            Xelian imports and runs it from source; it doesn&apos;t host the code.
          </p>
        </>
      ) : (
        <p className="mt-4 text-sm text-gray-500">Loading…</p>
      )}

      <hr className="my-8 border-gray-200" />

      {readme === null ? (
        <p className="text-sm text-gray-500">
          {entry ? "No README found." : ""}
        </p>
      ) : (
        <article className="prose prose-sm max-w-none prose-headings:font-semibold prose-a:text-blue-600">
          <ReactMarkdown
            remarkPlugins={[remarkGfm]}
            rehypePlugins={[rehypeSanitize]}
          >
            {readme}
          </ReactMarkdown>
        </article>
      )}
    </div>
  );
}
