"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useAuth } from "@/lib/auth";
import { publish } from "@/lib/api";

export default function PublishPage() {
  const { token, username, ready } = useAuth();
  const router = useRouter();
  const [name, setName] = useState("");
  const [archive, setArchive] = useState<File | null>(null);
  const [lockfile, setLockfile] = useState<File | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (ready && !token) {
    return (
      <div className="mx-auto max-w-sm px-4 py-16 text-center">
        <h1 className="text-xl font-semibold text-gray-900">
          Publish a package
        </h1>
        <p className="mt-3 text-sm text-gray-600">
          You need an account to publish.{" "}
          <Link href="/login" className="text-blue-600 underline">
            Log in
          </Link>{" "}
          or{" "}
          <Link href="/signup" className="text-blue-600 underline">
            sign up
          </Link>
          .
        </p>
      </div>
    );
  }

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!token || !username || !archive || !lockfile) return;
    setError(null);
    setBusy(true);
    try {
      const res = await publish(token, username, name.trim(), archive, lockfile);
      router.push(`/packages/${username}/${res.name}`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto max-w-lg px-4 py-12">
      <h1 className="text-xl font-semibold text-gray-900">Publish a package</h1>
      <p className="mt-1 text-sm text-gray-600">
        Upload the archive and lockfile produced by{" "}
        <code className="rounded bg-gray-100 px-1 py-0.5 font-mono text-xs">
          xelian build
        </code>
        . The registry verifies the checksum before accepting. Publishing to{" "}
        <span className="font-mono">{username ?? "…"}/</span> only.
      </p>

      <form onSubmit={submit} className="mt-6 flex flex-col gap-5">
        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-medium text-gray-700">
            Package name
          </span>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            pattern="[A-Za-z0-9._+-]+"
            title="Letters, digits, . _ + - only"
            placeholder="my-agent"
            className="rounded-md border border-gray-300 px-3 py-2 font-mono text-sm text-gray-900 focus:border-gray-500 focus:outline-none"
          />
        </label>

        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-medium text-gray-700">
            Package archive (.xelian)
          </span>
          <input
            type="file"
            accept=".xelian,.tar.gz,application/gzip"
            required
            onChange={(e) => setArchive(e.target.files?.[0] ?? null)}
            className="rounded-md border border-gray-300 px-3 py-2 text-sm text-gray-600 file:mr-3 file:rounded file:border-0 file:bg-gray-100 file:px-2.5 file:py-1 file:text-xs file:text-gray-700"
          />
        </label>

        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-medium text-gray-700">
            Lockfile (xelian.lock)
          </span>
          <input
            type="file"
            accept=".lock,.toml,text/plain"
            required
            onChange={(e) => setLockfile(e.target.files?.[0] ?? null)}
            className="rounded-md border border-gray-300 px-3 py-2 text-sm text-gray-600 file:mr-3 file:rounded file:border-0 file:bg-gray-100 file:px-2.5 file:py-1 file:text-xs file:text-gray-700"
          />
        </label>

        {error ? (
          <p className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
            {error}
          </p>
        ) : null}

        <button
          type="submit"
          disabled={busy || !ready}
          className="rounded-md bg-gray-900 px-3 py-2 text-sm font-medium text-white hover:bg-gray-700 disabled:opacity-60"
        >
          {busy ? "Publishing…" : "Publish"}
        </button>
      </form>

      <div className="mt-8 rounded-lg border border-gray-200 bg-gray-50 p-4">
        <h2 className="text-sm font-semibold text-gray-900">
          Prefer the CLI?
        </h2>
        <pre className="mt-2 overflow-x-auto font-mono text-xs text-gray-700">
          {`xelian login\nxelian push`}
        </pre>
      </div>
    </div>
  );
}
