// Client for the public Xelian registry API. The website is a read/write
// client of the exact same endpoints the CLI uses (SPEC §14.9) — no
// privileged control plane.

export const REGISTRY_URL = (
  process.env.NEXT_PUBLIC_REGISTRY_URL ?? "http://localhost:8000"
).replace(/\/$/, "");

export interface PackageSummary {
  owner: string;
  name: string;
  latest_version: string;
  description: string;
  package_type: string;
  language: string;
  license: string;
  tags: string[] | null;
  published_at: string;
}

export interface VersionRecord {
  version: string;
  checksum: string;
  published_at: string;
  yanked: boolean;
}

export interface AuthorInfo {
  name: string;
  email: string;
  homepage?: string | null;
}

export interface PackageInfo {
  owner: string;
  name: string;
  latest_version: string | null;
  description: string;
  package_type: string;
  language: string;
  runtime: string;
  entrypoint: string;
  license: string;
  permissions: string[];
  features: string[];
  author: AuthorInfo;
  readme: string;
  versions: VersionRecord[];
}

export interface LoginResponse {
  token: string;
  username: string;
}

async function fail(res: Response): Promise<never> {
  let detail = `${res.status} ${res.statusText}`;
  try {
    const body = await res.json();
    if (typeof body?.detail === "string") detail = body.detail;
  } catch {
    // non-JSON error body; keep the status line
  }
  throw new Error(detail);
}

export async function listPackages(): Promise<PackageSummary[]> {
  const res = await fetch(`${REGISTRY_URL}/packages`, { cache: "no-store" });
  if (!res.ok) return fail(res);
  return res.json();
}

export async function searchPackages(query: string): Promise<PackageSummary[]> {
  const res = await fetch(
    `${REGISTRY_URL}/search?q=${encodeURIComponent(query)}`,
    { cache: "no-store" },
  );
  if (!res.ok) return fail(res);
  return res.json();
}

export async function getPackage(
  owner: string,
  name: string,
): Promise<PackageInfo> {
  const res = await fetch(
    `${REGISTRY_URL}/packages/${encodeURIComponent(owner)}/${encodeURIComponent(name)}`,
    { cache: "no-store" },
  );
  if (!res.ok) return fail(res);
  return res.json();
}

export async function login(
  username: string,
  password: string,
): Promise<LoginResponse> {
  const res = await fetch(`${REGISTRY_URL}/auth/token`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ username, password }),
  });
  if (!res.ok) return fail(res);
  return res.json();
}

export async function signup(
  username: string,
  password: string,
): Promise<LoginResponse> {
  const res = await fetch(`${REGISTRY_URL}/auth/signup`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ username, password }),
  });
  if (!res.ok) return fail(res);
  return res.json();
}

export async function publish(
  token: string,
  owner: string,
  name: string,
  archive: File,
  lockfile: File,
): Promise<{ ok: boolean; name: string; version: string }> {
  const form = new FormData();
  form.append("archive", archive);
  form.append("lockfile", lockfile);
  form.append("owner", owner);
  form.append("name", name);
  const res = await fetch(`${REGISTRY_URL}/packages`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}` },
    body: form,
  });
  if (!res.ok) return fail(res);
  return res.json();
}
