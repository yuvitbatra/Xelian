"use client";

import { useEffect, useState } from "react";
import { useParams } from "next/navigation";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeSanitize from "rehype-sanitize";
import { getPackage, type PackageInfo } from "@/lib/api";
import CopyCommand from "@/components/copy-command";
import TypeBadge from "@/components/type-badge";

function Section({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <h3 className="text-xs font-semibold uppercase tracking-wide text-gray-500">
        {title}
      </h3>
      <div className="mt-2">{children}</div>
    </div>
  );
}

export default function PackagePage() {
  const params = useParams<{ owner: string; name: string }>();
  const [pkg, setPkg] = useState<PackageInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!params?.owner || !params?.name) return;
    getPackage(params.owner, params.name)
      .then(setPkg)
      .catch((e: Error) => setError(e.message));
  }, [params?.owner, params?.name]);

  if (error) {
    return (
      <div className="mx-auto max-w-6xl px-4 py-16 sm:px-6">
        <div className="rounded-lg border border-red-200 bg-red-50 p-4 text-sm text-red-700">
          {error}
        </div>
      </div>
    );
  }

  if (!pkg) {
    return (
      <p className="py-24 text-center text-sm text-gray-500">Loading…</p>
    );
  }

  return (
    <div className="mx-auto max-w-6xl px-4 py-8 sm:px-6">
      <div className="flex flex-wrap items-center gap-3">
        <h1 className="font-mono text-xl font-semibold text-gray-900">
          {pkg.owner}/{pkg.name}
        </h1>
        <TypeBadge type={pkg.package_type} />
        {pkg.latest_version ? (
          <span className="text-sm text-gray-500">v{pkg.latest_version}</span>
        ) : null}
      </div>
      {pkg.description ? (
        <p className="mt-2 max-w-3xl text-sm text-gray-600">
          {pkg.description}
        </p>
      ) : null}

      <div className="mt-6 grid grid-cols-1 gap-8 lg:grid-cols-[1fr_280px]">
        <div className="min-w-0 rounded-lg border border-gray-200 p-6">
          {pkg.readme ? (
            <article className="readme">
              <ReactMarkdown
                remarkPlugins={[remarkGfm]}
                rehypePlugins={[rehypeSanitize]}
              >
                {pkg.readme}
              </ReactMarkdown>
            </article>
          ) : (
            <p className="text-sm text-gray-500">
              This package has no README.
            </p>
          )}
        </div>

        <aside className="flex flex-col gap-6">
          <Section title="Install and run">
            <CopyCommand command={`xelian run ${pkg.owner}/${pkg.name}`} />
          </Section>

          <Section title="Permissions">
            {pkg.permissions.length === 0 ? (
              <p className="text-sm text-gray-500">None declared.</p>
            ) : (
              <ul className="flex flex-col gap-1.5">
                {pkg.permissions.map((perm) => (
                  <li
                    key={perm}
                    className="rounded-md border border-amber-200 bg-amber-50 px-2.5 py-1.5 font-mono text-xs text-amber-800"
                  >
                    {perm}
                  </li>
                ))}
              </ul>
            )}
            <p className="mt-2 text-xs text-gray-500">
              Xelian asks for consent before the first run.
            </p>
          </Section>

          <Section title="Features">
            {pkg.features.length === 0 ? (
              <p className="text-sm text-gray-500">None declared.</p>
            ) : (
              <div className="flex flex-wrap gap-1.5">
                {pkg.features.map((feat) => (
                  <span
                    key={feat}
                    className="rounded-full border border-gray-200 bg-gray-50 px-2.5 py-0.5 text-xs text-gray-700"
                  >
                    {feat}
                  </span>
                ))}
              </div>
            )}
          </Section>

          <Section title="Details">
            <dl className="flex flex-col gap-2 text-sm">
              {[
                ["Language", pkg.language],
                ["Runtime", pkg.runtime],
                ["License", pkg.license],
                ["Author", pkg.author?.name],
              ]
                .filter(([, v]) => v)
                .map(([label, value]) => (
                  <div key={label} className="flex justify-between gap-4">
                    <dt className="text-gray-500">{label}</dt>
                    <dd className="text-right text-gray-900">{value}</dd>
                  </div>
                ))}
            </dl>
          </Section>

          <Section title="Versions">
            <ul className="flex flex-col gap-1.5">
              {[...pkg.versions]
                .reverse()
                .map((v) => (
                  <li
                    key={v.version}
                    className="flex items-center justify-between rounded-md border border-gray-200 px-2.5 py-1.5 text-sm"
                  >
                    <span className="font-mono text-gray-900">
                      {v.version}
                    </span>
                    {v.yanked ? (
                      <span className="rounded bg-red-50 px-1.5 py-0.5 text-xs text-red-700">
                        yanked
                      </span>
                    ) : v.version === pkg.latest_version ? (
                      <span className="text-xs text-gray-500">latest</span>
                    ) : null}
                  </li>
                ))}
            </ul>
          </Section>
        </aside>
      </div>
    </div>
  );
}
