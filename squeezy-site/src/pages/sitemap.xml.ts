import { SITE } from "../config";

const routes = [
  "/",
  "/how-it-works/",
  "/docs/",
  "/docs/install/",
  "/docs/config/",
  "/docs/semantic-navigation/",
  "/docs/cost-receipts/",
  "/docs/permissions/",
  "/docs/troubleshooting/",
  "/benchmarks/",
  "/contact/"
];

export function GET() {
  const urls = routes
    .map((route) => {
      const loc = new URL(route, SITE.url).toString();
      return `<url><loc>${loc}</loc></url>`;
    })
    .join("");

  return new Response(`<?xml version="1.0" encoding="UTF-8"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">${urls}</urlset>`, {
    headers: {
      "Content-Type": "application/xml"
    }
  });
}
