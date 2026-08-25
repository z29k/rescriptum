---
title: Command line
description: Every subcommand and flag, and what each exit status means.
sidebar:
  label: Command line
  order: 4
---

# Command line

With no arguments, `rescriptum` runs the server. Everything else is a subcommand.

| Command | Does |
|---|---|
| `rescriptum` | run the server |
| `rescriptum render <id>` | print the answer that identifier would receive |
| `rescriptum render --body FILE` | …for a captured request body |
| `rescriptum render --query Q` | …for labels, e.g. `"mac=aa:bb&serial=7ABC1"` |
| `rescriptum check` | render everything in the configured store and report what breaks |
| `rescriptum import <dir>` | copy a directory of documents into the configured store |
| `rescriptum export <dir>` | write the configured store out as a directory of documents |
| `rescriptum --help` | usage and the environment variables |

All of them read the same [environment variables](./configuration.md), including
[`RESCRIPTUM_ENV_FILE`](./configuration.md#the-env-file) — which is resolved first, so a
file that cannot be read stops any command that needs configuration. `--help` and
`--version` are answered before it is read, because they are what you reach for when
something is wrong. There are no global flags.

## `render`

```console
$ rescriptum render 98:fa:9b:50:d8:10
$ rescriptum render --query "serial=7ABC123&mac=98:fa:9b:50:d8:10"
$ rescriptum render --query "path=/rhel/ks&serial=7ABC123"
$ rescriptum render --body /var/log/rescriptum-captures/2026…-0000.body
```

| Form | Facts it supplies |
|---|---|
| `<id>` | the identifier as a haystack, and nothing else — enough to match by name, not enough for a selector on `serial` |
| `--query "k=v&k2=v2"` | those labels, percent-decoded. `path=` also yields `file` and `segment`, and constrains the format the way a real URL would |
| `--body FILE` | the file verbatim: haystack, plus flattened JSON if it parses as JSON |

- The **document** goes to **stdout**; the `# format=… machine=… group=…` line explaining
  how it was reached goes to **stderr**. So `render … > answer.toml` gives you just the
  document.
- Load-time problems are printed as `warning:` lines first.
- Exit **0** when something resolved, **1** when nothing applied (the server would have
  returned `404`) or rendering failed.

## `check`

```console
$ rescriptum check
```

Reports load-time problems, renders every machine document and every group member, names
the groups that select on a `match` block (which it cannot try without a real request),
and calls the installer's own validator where one is on PATH.

Exit **0** when everything renders, **1** when anything failed — so it drops into CI as
is. See [validating](../answers/validating.md).

## `import` / `export`

```console
$ RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db rescriptum import /srv/answers
$ RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db rescriptum export /tmp/backup
```

`import` reads a **directory** and writes into the configured store; `export` does the
reverse. The round trip is byte-identical. Neither runs `check` for you — the output says
to.

## Exit statuses

| Status | Means |
|---|---|
| `0` | success |
| `1` | the command failed — nothing resolved, a document would not parse, the store could not be opened |

The server itself exits `0` on `SIGTERM` or Ctrl-C, and `1` if it cannot bind or cannot
open the store.
