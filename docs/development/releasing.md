---
title: Branching and releases
description: develop publishes nothing; a tag on main produces every binary. The exact sequence, and what CI refuses.
sidebar:
  label: Releasing
  order: 10
---

# Branching and releases

The model mirrors the sibling project `notabene` deliberately — same maintainer, same
expectations.

## Branches

| Branch | Rule |
|---|---|
| **`main`** | stable. Only release commits and `vX.Y.Z` tags land here. Never push feature work directly |
| **`develop`** | integration. Kept at the in-progress next version |
| **`feature/<name>`**, **`fix/<name>`** | branch from `develop`, PR back into `develop` |

```
main ──●────────────────────────●─(tag vX.Y.Z)──▶  releases
        \                      /
develop  ●───●───●───●───●────●  ────────────────▶  CI gates only, publishes nothing
          \     /   \       /
    feature/…  ●   fix/… ●          (PRs into develop)
```

**`develop` publishes nothing.** It runs the gates — build, tests, clippy, fmt — and stops
there. No prereleases, no artifacts. Binaries are produced **only** by a `vX.Y.Z` tag on
`main`.

That is the one thing that does *not* carry over from notabene, which is an npm package
and publishes prereleases to a `@dev` dist-tag. This project ships a compiled binary, so
the release artifact is a GitHub Release with cross-compiled binaries attached, built by a
CI matrix.

## Commits

Conventional commits **with a scope**:

```
feat(http): answer GET as well as POST
fix(select): normalize member strings before comparing
chore: release v0.2.0
```

Keep PRs focused. Adding a dependency needs a reason in the commit message — this binary
runs as root on other people's hardware.

## Cutting a release

```bash
# on develop, with everything green
$EDITOR Cargo.toml          # bump version
cargo build                 # refresh Cargo.lock
git commit -am "chore: release vX.Y.Z"

git checkout main && git merge --no-ff develop
git tag -a vX.Y.Z -m "rescriptum vX.Y.Z"
git push origin main --follow-tags
```

`.github/workflows/release.yml` then:

1. **Refuses the tag if it disagrees with `Cargo.toml`.** A release whose binary reports a
   different version than its tag is a support problem that outlives the release.
2. Cross-compiles the [five published targets](./building.md#the-release-targets).
3. Packages each as `rescriptum-<version>-<target>.tar.gz`, with `README.md` and `LICENSE`
   alongside the binary, plus a **SHA-256 sum** — whoever runs this as root should be able
   to check what they downloaded.
4. Wraps the Linux musl builds as [Synology packages](./building.md#the-synology-package),
   `rescriptum-<version>-<build>-<abi>.spk`, and checks each structurally before it can be
   published.
5. Cuts the GitHub Release with `gh` and `--generate-notes`, or uploads into it if it
   already exists.

It is re-runnable by hand through `workflow_dispatch` with a tag, for when a job fails
after the tag is already pushed.

**A packaging-only fix needs no tag.** SPK versions are all-numeric segments and the last
one is a package build number, so `v0.1.0` produces `0.1.0-1`; dispatching by hand with
`spk_build: 2` attaches `rescriptum-0.1.0-2-<abi>.spk` to the same Release. A prerelease
does not produce an `.spk` at all — the archives are the prerelease channel.

**A tag must not be the first time an `.spk` is installed on a DSM machine.** The
structural check catches a broken archive; only Package Center catches a broken package,
and the first published one is the one whose uninstall scripts will run during everybody's
first upgrade. The checklist is in
[`packaging/dsm/README.md`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/README.md).

Every action used is an official `actions/*` one, and `gh` is already on the runner. That
is deliberate for the same reason as everything else in this file.

## Versioning

SemVer. The tag is `vX.Y.Z` and must match `Cargo.toml` exactly.

Answer documents are data, not state: nothing migrates, and a new binary reads the same
directory. The exception is the **SQLite schema**, which carries a `user_version` — see
[stores](./stores.md#the-sqlite-store). There is one version so far. Adding a second means
writing the migration step *and* a minor bump at least, and the release notes have to say
so, because an older binary will refuse the upgraded database rather than half-read it.

## Documentation

The [documentation site](./docs-site.md) is published from **`main`**, so a docs change
ships with the next release — or by running the `docs` workflow by hand
(`workflow_dispatch`) when it should not wait.
