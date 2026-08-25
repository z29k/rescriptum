---
title: The documentation site
description: How docs/ is written, reviewed with notabene, gated in CI, and published to GitHub Pages.
sidebar:
  label: Documentation site
  order: 12
---

# The documentation site

This site **is** `docs/` in the repository, rendered by
[notabene](https://z29k.github.io/notabene/) and published to GitHub Pages. The Rust
binary knows nothing about any of it; the docs toolchain is a `package.json` and one
config file, and removing it would leave `docs/` as perfectly readable Markdown.

## Why a site and not a longer README

The README had grown to 28 KB and was three documents wearing one coat: a pitch, a user
manual, and an architecture note. A reader looking for the DSM firewall step had to scroll
past the merge semantics. So:

- **`docs/guide/`** — using rescriptum: install, write answers, run it in production.
- **`docs/development/`** — building rescriptum: the constraints, the internals, the
  release.
- **`README.md`** — what it is, a 30-second demonstration, and links into the site.

The two spaces have different audiences and no reason to interleave.

## Two languages

The site is bilingual: **English is the source**, French is a translation of it. The
`suffix` i18n strategy means the English files keep their paths and URLs and the French
ones are `*.fr.md` siblings:

```
docs/guide/answers/grouping.md      → /guide/answers/grouping
docs/guide/answers/grouping.fr.md   → /fr/guide/answers/grouping
```

That layout was chosen over a folder per locale because it can be added to an existing
doc without moving anything — the English URLs and their comment threads survive.

Rules that follow from it:

- **Write English first**, then translate. A change to an English page that is not
  mirrored leaves the French page stale rather than broken; the reader falls back with a
  banner.
- **Links keep the base name.** From a French page, write `./selection.md`, not
  `./selection.fr.md` — notabene resolves the locale. But **anchors must be the French
  heading's slug**: `./templating.md#machine-exige-un-document-machine`.
- **Comments are per language.** A comment left on the French page is its own thread and
  maps to the French source file.
- The site chrome, search and `llms.txt` are per locale too.

`README.md` and `README.fr.md` follow the same rule and link to each other.

## Working on the docs

```bash
npm install          # once
npm run docs         # → http://localhost:3009
```

That opens the site with the **review loop** enabled: select any text on the rendered page
and leave a comment, exactly where the problem is. The anchored comment *is* the
instruction — no quoting a passage into a chat box and hoping the agent re-finds it.

Then tell your agent **"address the doc comments"**. It reads `docs/.notabene/`, edits the
source, marks each comment handled, and appends a journal entry saying what changed and
why.

| Script | Does |
|---|---|
| `npm run docs` | the review server, live-reloading |
| `npm run docs:build` | the public static site into `./_site` |
| `npm run docs:preview` | serve what was built |
| `npm run docs:lint` | validate every internal link against the routes the last build emitted |
| `npm run docs:status` / `docs:stop` | manage a detached dev server |

Using Claude Code? `/plugin marketplace add z29k/notabene` then
`/plugin install notabene@z29k`, and say *"set up notabene"*. The plugin runs its own
pinned renderer, so it does not conflict with the one in `package.json`.

### Review mode is `approve`

`notabene.config.mjs` sets `review: "approve"`, so the agent **proposes** rather than
resolves: each edit is validated against its **real git diff** at `/review` before the
comment is closed. Documentation that describes root passwords and boot-time configuration
is worth reading before it ships. Change it to `"auto"` if that ceremony is not earning
its keep.

## Writing a page

Every page is CommonMark with optional YAML frontmatter:

```yaml
---
title: Groups and merging
description: One sentence — it becomes the meta description and the search snippet.
sidebar:
  label: Grouping        # sidebar text, if the title is too long for it
  order: 3               # position among siblings, ascending
---
```

Everything has a default: a page with no frontmatter renders fine, ordered
alphabetically. A folder is named and positioned by its `index.md`.

Conventions in this repository:

- **Relative links between pages**, with the `.md` extension — `./selection.md`,
  `../reference/configuration.md`. They become routes on the site and stay clickable on
  GitHub.
- **Absolute GitHub URLs for repository files** — `answers/`, `CLAUDE.md`, a workflow.
  They are outside `docs/` and have no route.
- **English**, like everything else written to disk here.
- **Mermaid diagrams** are rendered natively, in a ` ```mermaid ` fence — see
  [architecture](./architecture.md) and [the request lifecycle](./request-lifecycle.md).
- **Every page needs its `*.fr.md` sibling**, with the frontmatter translated too — the
  `title`, `description` and `sidebar.label` are all reader-facing.

## Configuration

`notabene.config.mjs` at the repository root. The parts that matter:

```js
roots: [
  { key: "guide",       label: "Guide",                              path: "docs/guide" },
  { key: "development", label: { en: "Development", fr: "Développement" }, path: "docs/development" },
],
store: "docs/.notabene",
home: { en: "docs/home.md", fr: "docs/home.fr.md" },
i18n: { locales: ["en", "fr"], defaultLocale: "en", strategy: "suffix" },
branding: {
  logo: "assets/rescriptum-logo.jpg",
  favicon: "assets/rescriptum-logo.jpg",
  socialImage: "assets/rescriptum-logo.jpg",
},
editPattern: "https://github.com/z29k/rescriptum/edit/develop/{path}",
review: "approve",
publish: { site: "https://z29k.github.io", base: "/rescriptum" },
```

Every reader-facing string in the config takes a per-locale map — a space's `label` and
`description`, the home page, every nav link label, the sidebar block title, the footer.
Unset for a locale, it falls back to the default one.

The logo is `assets/rescriptum-logo.jpg`: a sealed rescript on a floppy disk — a written
answer, delivered by a machine. It serves as the topbar logo, the favicon and the social
card, and it is the image the README uses too.

`editPattern` points at **`develop`**, not `main`: docs are merged there like everything
else, and `main` only receives release commits.

`docs/.notabene/` is the comment and journal store — plain JSON, committed, diffable in a
PR. Commit it.

## The CI gate

`.github/workflows/ci.yml` has a `docs` job: `npm ci`, build the public site, then
`notabene lint`, which checks every internal link against the routes the build **actually
emitted** and suggests near-misses. A dead link in published documentation is cheap to
prevent and embarrassing to ship.

It runs on the same pushes as the Rust gates.

## Publishing

`.github/workflows/docs.yml` builds `--public` and deploys to GitHub Pages on every push
to **`main`** that touches `docs/`, the config, or the workflow — plus
`workflow_dispatch`, for publishing a docs fix without waiting for a release.

Because `main` only receives release commits, **documentation normally ships with a
release.** Run the workflow by hand when it should not wait.

The artifact is the read-only public build: no review UI, no store data, plus `llms.txt`,
a Markdown twin per page, a sitemap and OpenGraph metadata. `pagefind` is a dev dependency,
so `npm ci` gives the site full-text search with no further configuration.

One-time repository setup: **Settings → Pages → Source = GitHub Actions**.

## The dependency situation

`npm audit` reports advisories in Astro, esbuild and sharp, transitively under notabene,
with no fixes currently available upstream. They are **development-only**: nothing from
`node_modules` is executed by the published site or reaches the Rust binary, and the CI
job builds static HTML from Markdown this repository owns.

Worth re-checking when notabene updates, not worth blocking on.
