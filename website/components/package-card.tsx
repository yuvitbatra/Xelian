import Link from "next/link";
import type { PackageSummary } from "@/lib/api";
import TypeBadge from "./type-badge";

export default function PackageCard({ pkg }: { pkg: PackageSummary }) {
  return (
    <Link
      href={`/packages/${pkg.owner}/${pkg.name}`}
      className="block rounded-lg border border-gray-200 bg-white p-4 transition-colors hover:border-gray-300 hover:bg-gray-50"
    >
      <div className="flex items-start justify-between gap-2">
        <span className="truncate font-mono text-sm font-medium text-gray-900">
          {pkg.owner}/{pkg.name}
        </span>
        <TypeBadge type={pkg.package_type} />
      </div>
      {pkg.description ? (
        <p className="mt-2 line-clamp-2 text-sm text-gray-600">
          {pkg.description}
        </p>
      ) : null}
      <div className="mt-3 flex items-center gap-3 text-xs text-gray-500">
        <span>v{pkg.latest_version}</span>
        {pkg.language ? <span>{pkg.language}</span> : null}
        {pkg.license ? <span>{pkg.license}</span> : null}
        {(pkg.tags ?? []).slice(0, 3).map((tag) => (
          <span
            key={tag}
            className="rounded bg-gray-100 px-1.5 py-0.5 text-gray-600"
          >
            {tag}
          </span>
        ))}
      </div>
    </Link>
  );
}
