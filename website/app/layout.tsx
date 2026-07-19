import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";
import { AuthProvider } from "@/lib/auth";
import Header from "@/components/header";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "Xelian — run AI agents like models",
  description:
    "Xelian is a local-first registry and runtime for AI agents and MCP servers. Package once, publish, and anyone can run it with a single command.",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="en"
      className={`${geistSans.variable} ${geistMono.variable} h-full antialiased`}
    >
      <body className="flex min-h-full flex-col font-sans">
        <AuthProvider>
          <Header />
          <main className="flex-1">{children}</main>
          <footer className="border-t border-gray-200 py-8">
            <div className="mx-auto flex max-w-6xl items-center justify-between px-4 text-sm text-gray-500 sm:px-6">
              <span>Xelian — a local-first registry for AI agents</span>
              <span className="font-mono text-xs">
                xelian run owner/package
              </span>
            </div>
          </footer>
        </AuthProvider>
      </body>
    </html>
  );
}
