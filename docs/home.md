# rescriptum

**An HTTP server that compiles and renders the configuration files for automated OS installs.**
It answers any unattended installer: Proxmox, Debian, RHEL, Ubuntu, Flatcar, SUSE, Windows.
It recognises the machine from its MAC address, serial number or hardware inventory, stacks
the layers that apply to it, and returns the result. Whatever a machine is about to receive,
you can read it before you power the machine on.

```console
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers rescriptum
2026-08-22T18:00:00Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:/srv/answers workers=8 max_conn=2048 timeout=10s
2026-08-22T18:00:04Z 10.0.0.42:51234 POST /answer body=1876 200 format=toml machine=98-fa-9b-50-d8-10 group=rack-a bytes=412
```

One static binary, no runtime, no container — as happy on a 512 MB ARM NAS as on a
datacenter host fielding a provisioning burst.

## Start here

- **[What rescriptum is](./guide/index.md)** — the problem it solves and the shape of
  the solution, in five minutes.
- **[Install](./guide/install.md)** then **[serve your first answer](./guide/quickstart.md)** —
  from a downloaded binary to a machine receiving its own configuration.
- **[Preparing installer media](./guide/iso.md)** — the URL to bake into each ISO, per
  operating system.

## Writing answers

- **[How an answer is picked](./guide/answers/selection.md)** — by name, by member list,
  or by what the machine *is*.
- **[One document per operating system](./guide/answers/formats.md)** — the extension is
  the format, the endpoint chooses between them.
- **[Groups and merging](./guide/answers/grouping.md)** — a rack shares one file; a
  machine that differs carries only its difference.
- **[Templating](./guide/answers/templating.md)** — `{{ serial }}` in a group covers five
  hundred machines.
- **[Validating what will be served](./guide/answers/validating.md)** — `render` and
  `check`, because a merged answer is a document nobody ever wrote.

## Running it

- **[Deployment](./guide/operations/deployment.md)** and
  **[Synology DSM 7](./guide/operations/synology.md)**.
- **[Security](./guide/operations/security.md)** — what the tokens protect and what they
  do not.
- **[The SQLite store](./guide/operations/sqlite.md)** and
  **[the admin API](./guide/operations/admin-api.md)** — for a fleet administered by
  tooling rather than by hand.
- **[Troubleshooting](./guide/operations/troubleshooting.md)** — the log line is the
  whole diagnostic story.

Exhaustive tables live in the [Reference](./guide/reference/index.md): every
[environment variable](./guide/reference/configuration.md), the
[HTTP surface](./guide/reference/endpoints.md), the
[format and endpoint tables](./guide/reference/formats.md), and the
[command line](./guide/reference/cli.md).

## Working on rescriptum

The [Development](./development/index.md) space is the other half of this site: the
[constraints](./development/constraints.md) that shape the code and why they are not
negotiable, the [request lifecycle](./development/request-lifecycle.md), the internals of
[selection](./development/selection.md), [formats](./development/formats.md) and
[stores](./development/stores.md), how the [tests](./development/testing.md) are
organised, and how a [release](./development/releasing.md) is cut.
