# Cloudflare Deployment

The documentation is a standalone VitePress site located in `docs`.

## Cloudflare Pages settings

Create a Pages project connected to the GitHub repository and configure:

| Setting | Value |
| --- | --- |
| Production branch | `bridge` |
| Root directory | `docs` |
| Build command | `pnpm build` |
| Build output directory | `.vitepress/dist` |

Cloudflare installs dependencies from `docs/package.json` and publishes the
static VitePress output.

## Local development

From the repository root:

```sh
cd docs
pnpm install
pnpm dev
```

Build the production site:

```sh
pnpm build
pnpm preview
```
