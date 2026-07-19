"use client";

import { useState } from "react";
import Link from "next/link";
import { useRouter } from "next/navigation";
import { useAuth } from "@/lib/auth";
import { login, signup } from "@/lib/api";

export default function AuthForm({ mode }: { mode: "login" | "signup" }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { signIn } = useAuth();
  const router = useRouter();

  const isSignup = mode === "signup";

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setError(null);
    setBusy(true);
    try {
      const call = isSignup ? signup : login;
      const res = await call(username, password);
      signIn(res.token, res.username);
      router.push("/");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mx-auto max-w-sm px-4 py-16">
      <h1 className="text-xl font-semibold text-gray-900">
        {isSignup ? "Create an account" : "Log in"}
      </h1>
      <p className="mt-1 text-sm text-gray-600">
        {isSignup
          ? "Your username becomes your publish namespace."
          : "Log in to publish packages."}
      </p>
      <form onSubmit={submit} className="mt-6 flex flex-col gap-4">
        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-medium text-gray-700">Username</span>
          <input
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            required
            autoComplete="username"
            pattern={isSignup ? "[A-Za-z0-9][A-Za-z0-9._-]{1,38}" : undefined}
            title={
              isSignup
                ? "2-39 characters: letters, digits, . _ -, starting with a letter or digit"
                : undefined
            }
            className="rounded-md border border-gray-300 px-3 py-2 text-sm text-gray-900 focus:border-gray-500 focus:outline-none"
          />
        </label>
        <label className="flex flex-col gap-1.5">
          <span className="text-sm font-medium text-gray-700">Password</span>
          <input
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
            minLength={isSignup ? 8 : undefined}
            autoComplete={isSignup ? "new-password" : "current-password"}
            className="rounded-md border border-gray-300 px-3 py-2 text-sm text-gray-900 focus:border-gray-500 focus:outline-none"
          />
          {isSignup ? (
            <span className="text-xs text-gray-500">
              At least 8 characters.
            </span>
          ) : null}
        </label>
        {error ? (
          <p className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
            {error}
          </p>
        ) : null}
        <button
          type="submit"
          disabled={busy}
          className="rounded-md bg-gray-900 px-3 py-2 text-sm font-medium text-white hover:bg-gray-700 disabled:opacity-60"
        >
          {busy ? "Working…" : isSignup ? "Sign up" : "Log in"}
        </button>
      </form>
      <p className="mt-4 text-sm text-gray-600">
        {isSignup ? (
          <>
            Already have an account?{" "}
            <Link href="/login" className="text-blue-600 underline">
              Log in
            </Link>
          </>
        ) : (
          <>
            No account?{" "}
            <Link href="/signup" className="text-blue-600 underline">
              Sign up
            </Link>
          </>
        )}
      </p>
    </div>
  );
}
