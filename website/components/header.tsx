"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useAuth } from "@/lib/auth";

export default function Header() {
  const { username, ready, signOut } = useAuth();
  const router = useRouter();

  return (
    <header className="border-b border-gray-200 bg-white">
      <div className="mx-auto flex h-14 max-w-6xl items-center justify-between px-4 sm:px-6">
        <div className="flex items-center gap-8">
          <Link href="/" className="flex items-center gap-2">
            <span className="flex h-7 w-7 items-center justify-center rounded-md bg-gray-900 font-mono text-sm font-bold text-white">
              X
            </span>
            <span className="text-[15px] font-semibold tracking-tight text-gray-900">
              Xelian
            </span>
          </Link>
          <nav className="hidden items-center gap-6 sm:flex">
            <Link
              href="/"
              className="text-sm text-gray-600 hover:text-gray-900"
            >
              Packages
            </Link>
            <Link
              href="/new"
              className="text-sm text-gray-600 hover:text-gray-900"
            >
              Publish
            </Link>
          </nav>
        </div>
        <div className="flex items-center gap-3">
          {!ready ? null : username ? (
            <>
              <span className="text-sm text-gray-600">
                <span className="font-medium text-gray-900">{username}</span>
              </span>
              <button
                onClick={() => {
                  signOut();
                  router.push("/");
                }}
                className="rounded-md border border-gray-300 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
              >
                Sign out
              </button>
            </>
          ) : (
            <>
              <Link
                href="/login"
                className="rounded-md px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50"
              >
                Log in
              </Link>
              <Link
                href="/signup"
                className="rounded-md bg-gray-900 px-3 py-1.5 text-sm font-medium text-white hover:bg-gray-700"
              >
                Sign up
              </Link>
            </>
          )}
        </div>
      </div>
    </header>
  );
}
