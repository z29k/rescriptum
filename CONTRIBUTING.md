# Contributing to rescriptum

Thanks for looking. rescriptum is a small, focused thing: it serves unattended-installation
answers, composed per machine, to whichever installer asks. Contributions are welcome.

## Getting oriented

Read the [Development documentation](https://z29k.github.io/rescriptum/development/)
first — the constraints and why they are not negotiable, the request lifecycle, the
internals, and a [list of traps](https://z29k.github.io/rescriptum/development/traps) so
nobody hits them twice. [`CLAUDE.md`](CLAUDE.md) is the same material condensed for coding
agents.

The short version:

- `src/facts.rs` — what a request tells us about the machine.
- `src/format/` — one interface per document format; `xml.rs` holds the XML tree.
- `src/select.rs` — matching, layering, and what actually gets served.
- `src/store/` — where documents come from: a directory of files, or SQLite.
- `src/admin.rs` — the write API, and the guarantee that a write cannot break the fleet.

## Develop

```bash
cargo test                      # 308 tests
cargo test <name>               # one, by substring
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all

./build.sh                      # this machine
./build.sh --all                # every target a release ships
./build.sh --help
```

Cross-compiling needs [`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) and
Zig; the README explains the setup.

Try a change against the example answers rather than only against tests:

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- render --query "path=/rhel/ks&serial=7ABC123"
```

## Documentation

`docs/` is the documentation site, rendered by [notabene](https://z29k.github.io/notabene/)
and published to GitHub Pages from `main`. It has two spaces: `docs/guide/` for using
rescriptum, `docs/development/` for working on it.

```bash
npm install
npm run docs          # → http://localhost:3009, with review comments on the page
npm run docs:build    # the public artifact
npm run docs:lint     # no broken internal links — CI runs this too
```

Leave a comment on the rendered page and an agent can apply it; the store lives in
`docs/.notabene/` and is committed. Details in
[the documentation site](https://z29k.github.io/rescriptum/development/docs-site).

The docs are **bilingual**: English is the source, French is a `*.fr.md` sibling of every
page. Write the English first, then translate — see
[the documentation site](https://z29k.github.io/rescriptum/development/docs-site#two-languages).

A user-visible change should land with its documentation in the same PR.

## Conventions

- **English** for code, comments, commit messages, and the source of the documentation
  (which is additionally published in French).
- **A behaviour belongs in `tests/stores.rs`**, which runs every case against both the file
  and SQLite stores and requires the same result. That suite is what keeps two backends from
  drifting; a test that only covers one of them proves half of what it claims.
- **Arrays replace, they do not append**, in every format. A list that can only grow cannot
  be shortened from a higher layer.
- **Fail loudly.** A missing group, an unfillable template, a document that will not parse —
  all are errors with a reason. Serving a half-built answer installs a machine wrongly, and
  nobody finds out until it is running.
- **Adding a dependency needs a reason in the commit message.** This binary runs as root on
  other people's hardware. CI's `audit` job is the other half of that rule: `cargo audit
  --deny warnings` over the whole tree, which fails on an unmaintained or yanked crate as
  well as on a vulnerability.

## Branching

- **`main`** — stable. Only release commits and `vX.Y.Z` tags land here; never push feature
  work to it directly.
- **`develop`** — integration. Kept at the in-progress next version.
- **`feature/<name>`** and **`fix/<name>`** — branch from `develop`, open a PR back into it.

```
main ──●────────────────────────●─(tag vX.Y.Z)──▶  releases
        \                      /
develop  ●───●───●───●───●────●  ────────────────▶  CI gates only, publishes nothing
          \     /   \       /
    feature/…  ●   fix/… ●          (PRs into develop)
```

## Releasing

`develop` publishes nothing — it runs the gates and stops. Binaries come only from a tag on
`main`:

```bash
# on develop, with everything green
$EDITOR Cargo.toml          # bump version
cargo build                 # refresh Cargo.lock
git commit -am "chore: release vX.Y.Z"

git checkout main && git merge --no-ff develop
git tag -a vX.Y.Z -m "rescriptum vX.Y.Z"
git push origin main --follow-tags
```

`.github/workflows/release.yml` then refuses the tag if it disagrees with `Cargo.toml`,
cross-compiles the five published targets, and attaches the archives and their SHA-256 sums
to a GitHub Release.

Every action used in CI is an official `actions/*` one, and the release is cut with the
`gh` CLI already on the runner. That is deliberate: this toolchain links a binary people run
as root.

## Reporting something

For a wrong answer, the useful report is what the machine sent and what it got back. Set
`RESCRIPTUM_CAPTURE_DIR` and the server records both:

```bash
rescriptum render --body captured/2026-…-0000.body
```

Scrub the password hashes and SSH keys before attaching anything.
