# SqueezyAgent Site

Draft static website for `squeezyagent.com`.

This site is intentionally separate from the Rust implementation repo while the
product is pre-v0. Content is allowed to evolve as implementation behavior,
docs, benchmarks, and contact paths become real.

## Local Development

```sh
npm install
npm run dev
```

The local dev server defaults to:

```text
http://127.0.0.1:4321/
```

## Build

```sh
npm run build
```

Astro writes the static site to `dist/`.

## Cloudflare Pages

Recommended Pages settings:

```text
Framework preset: Astro
Build command: npm run build
Build output directory: dist
Root directory: squeezy-site
Production branch: main, or the branch used for the website draft
```

You do not need Cloudflare credentials to edit or build this template locally.
Publishing to `squeezyagent.com` later requires:

- a Git repository connected to Cloudflare Pages, or a manual Pages upload;
- a Cloudflare Pages project;
- `squeezyagent.com` added as a custom domain on that Pages project;
- optional `www.squeezyagent.com` redirect or alias.

Because the domain is already in Cloudflare, Pages can usually create or update
the necessary DNS records after the custom domain is added.

## Content Status

- Homepage: draft positioning.
- How it works: architecture narrative, pre-v0.
- Docs: split draft docs under `/docs/` for install, config, semantic
  navigation, cost receipts, permissions, and troubleshooting.
- Roadmap: evolving pre-v0 roadmap.
- Benchmarks: methodology placeholder, no public savings claims yet.
- Contact: GitHub-first placeholder, with security email to be added before a
  public release.

Shared site metadata, GitHub URLs, and docs navigation live in
`src/config.ts`.

Static assets include:

- `public/favicon.svg`
- `public/og.svg`
- `public/robots.txt`
- `public/site.webmanifest`
- generated `/sitemap.xml`
