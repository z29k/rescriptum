# Agent instructions

<!-- notabene:begin -->
## Documentation review (notabene)

Review comments on this repo's docs live in `docs/.notabene/` — one JSON file per comment,
committed with the code. When asked to "address the doc comments" (or anything
equivalent), follow the notabene review protocol:

- **The protocol**: `docs/.notabene/protocol.md` — read it first; it is the
  complete spec (store layout, anchor resolution, journaling, verification).
- Online copy: <https://z29k.github.io/notabene/reference/agent-protocol/>

Non-negotiable: only process comments with `status: "open"` and `hold: false`; never
delete the store; never commit without being asked.
<!-- notabene:end -->

## Changing the Synology package

`packaging/dsm/` is shell that runs as root-adjacent code on someone's NAS, and **the local
harness cannot prove it**. Two of the three real defects found so far were invisible to a
fake-tree test and only appeared on a real DSM.

- The procedure is
  [`packaging/dsm/vm/README.md`](packaging/dsm/vm/README.md) → *Changing the package? This is
  the procedure*. Follow it rather than inventing a shortcut.
- A **DSM 7.2.2 virtual machine already exists on the maintainer's machine**, in Docker, with
  a `clean` snapshot to restore. It is set up by `packaging/dsm/vm/bootstrap.sh` and driven
  by `packaging/dsm/vm/on-dsm.sh`; neither needs anything outside Docker.
- The machine checks are **destructive on purpose** — they upgrade over a hand-edited
  configuration and then uninstall. Restore the snapshot before and after.
- **Never name a DSM account after the package user** (`rescriptum`): DSM deletes it with the
  package. The rig's account is `rigadmin`.
