import type { MetadataRoute } from "next";

const SITE_URL = "https://xelian.vercel.app";

export default function sitemap(): MetadataRoute.Sitemap {
  const now = new Date();
  const routes = ["", "/explore", "/new", "/login", "/signup"];
  return routes.map((path) => ({
    url: `${SITE_URL}${path}`,
    lastModified: now,
    changeFrequency: path === "/explore" ? "daily" : "weekly",
    priority: path === "" ? 1 : 0.7,
  }));
}
