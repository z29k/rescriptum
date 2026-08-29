# CLAUDE.md — docs

Guidance for working on the documentation site. It loads only when Claude touches files under
`docs/`. The root `CLAUDE.md` keeps what applies everywhere: the site exists, this file and
`docs/development/` overlap deliberately, and **a change to a constraint belongs in both**.

## Documentation

`docs/` is the documentation site, rendered by **notabene** (`@z29k/notabene`, the sibling
project) and published to GitHub Pages at <https://z29k.github.io/rescriptum/>. Two spaces,
two audiences, and they do not interleave:

- `docs/guide/` — **using** rescriptum: install, quick start, installer media, writing
  answers (`answers/`), running it (`operations/`), exhaustive tables (`reference/`).
- `docs/development/` — **working on** rescriptum: constraints, architecture, request
  lifecycle, internals per module, testing, building, releasing, traps.

This file and `docs/development/` overlap deliberately: this one is condensed for agents,
that one is written for a human reading in order. **A change to a constraint belongs in
both.** A user-visible change should land with its documentation in the same PR.

- `notabene.config.mjs` — the site configuration. `review: "approve"` means an agent
  *proposes* doc edits and a human validates each against its real git diff at `/review`.
  `i18n: { locales: ["en","fr"], strategy: "suffix" }` is what makes the FR siblings work.
  **`tagline` is a plain string, not a per-locale map** — notabene does not accept a map
  there, and one stringifies to `[object Object]` in the topbar and `llms.txt`.
- `assets/rescriptum-logo.jpg` — the logo (a sealed rescript on a floppy disk), used as the
  topbar logo, the favicon, the social card, and the README header.
- `docs/.notabene/` — the comment and journal store, plain JSON, **committed**. The agent
  protocol is `docs/.notabene/protocol.md`, pointed at from `AGENTS.md`.
- `package.json` exists **only** for this. Nothing in `node_modules` is executed by the
  published site or reaches the Rust binary. `npm audit` reports unfixable transitive
  advisories in Astro/esbuild/sharp; they are development-only.
- `.github/workflows/docs.yml` publishes from `main` (so docs normally ship with a
  release; `workflow_dispatch` publishes a fix that should not wait). `ci.yml` has a
  `docs` job that builds and runs `notabene lint` on every push.

Writing conventions: relative `.md` links between pages (they become routes *and* stay
clickable on GitHub), absolute GitHub URLs for repository files outside `docs/`, frontmatter
`title` / `description` / `sidebar.order`, Mermaid in ` ```mermaid ` fences.

From a French page, a link still uses the **base** name (`./selection.md`, never
`./selection.fr.md`) — notabene resolves the locale — but the **anchor must be the French
heading's slug**. `notabene lint` checks routes, **not anchors**: verify those against the
built HTML.

**Verify prose against the binary rather than against this file.** Every command output in
`docs/` was captured from a real run; several passages in the older README had drifted from
what the code actually prints.
