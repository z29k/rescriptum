---
title: Validating what will be served
description: A merged answer is a document nobody ever wrote. render prints it; check renders everything and reports what breaks.
sidebar:
  label: Validating
  order: 5
---

# Validating what will be served

Before answers composed, an admin wrote a complete file and validated it:

```console
$ proxmox-auto-install-assistant validate-answer answer.toml
```

Once an answer is assembled from a group chain plus a machine document plus a template
fill, **the file the installer receives is one nobody has ever seen** — and a bad merge
surfaces as a failed unattended install at 3am. Two subcommands exist to close that gap,
and any change to merging has to keep them working.

## `render` — what this machine would get

```console
$ rescriptum render 98:fa:9b:50:d8:10                              # by identity
$ rescriptum render --query "serial=7ABC123&mac=98:fa:9b:50:d8:10" # by label
$ rescriptum render --query "path=/rhel/ks&serial=7ABC123"         # including the endpoint
$ rescriptum render --body captured-request.json                   # a real captured body
```

It resolves exactly as the server does — same matching, same layering, same template fill
— and prints the result. The **document goes to stdout**; the line explaining how it was
reached goes to **stderr**:

```console
$ rescriptum render 98:fa:9b:50:d8:10
# format=toml machine=98-fa-9b-50-d8-10 group=rack-a
[global]
…
```

so redirecting gives you just the document:

```console
$ rescriptum render 98:fa:9b:50:d8:10 > /tmp/answer.toml
```

Add `path=…` to `--query` when you want to check what a *particular endpoint* would
answer — without it, resolution is unconstrained by format and may pick a document the
real URL would have excluded.

Exit status is 0 when something resolved, non-zero when nothing applied (the server would
have returned 404) or when rendering failed.

## `check` — render everything, report what breaks

```console
$ rescriptum check
checking files:examples
  10 group(s), 8 machine file(s)
  group "rhel-compute" selects on serial=7ABC*
    (verify with: rescriptum render --query "...")
  group "ubuntu-web" selects on file=user-data product=PowerEdge R6*
    (verify with: rescriptum render --query "...")
  1 answer(s) validated by their installer's own tool
  note: no schema validator exists for preseed answers
  note: toml answers not schema-checked — proxmox-auto-install-assistant is not on PATH
  ok — everything renders

Well-formed and merging cleanly is not the same as valid for an
installer. Where a validator exists and is installed it was used above;
install proxmox-auto-install-assistant, xmllint or ksvalidator for the rest.
```

What it does:

- **Reports load-time problems** — a group extending one that does not exist, a cycle
  between groups, a document that will not parse.
- **Renders every machine document**, and every member of every group. That is what
  actually exercises the merge.
- **Names the groups that select on a `match` block** and says it could not try them,
  rather than implying they were verified — a selector needs a real request.
- **Flags a group with neither `members` nor `match`** as reachable only via `extends`,
  in case that was not the intention.
- **Calls the installer's own validator** where one exists and is on PATH, and says which
  formats it could not check.

Exit status is 0 when everything renders, 1 when anything failed — so it drops straight
into CI.

### The validators it knows

| Format | Tool |
|---|---|
| `toml` | `proxmox-auto-install-assistant validate-answer` |
| `xml`, `autoyast`, `unattend` | `xmllint --noout` |
| `ks` | `ksvalidator` |
| `yaml`, `json`, `ign`, `preseed`, `cfg`, `ipxe` | none exists — render and read it |

A missing tool is reported once as a note, never treated as a failure. A checker that
refuses to run without optional tooling is a checker nobody runs.

`check` is not a schema checker itself: it proves your documents are well-formed and
merge cleanly. For anything it cannot call a validator for, pipe a rendered answer in
yourself:

```console
$ rescriptum render 98:fa:9b:50:d8:10 > /tmp/answer.toml
$ proxmox-auto-install-assistant validate-answer /tmp/answer.toml
```

### What `check` cannot prove

`check` renders each machine from its **identity alone**. It has no request, so it cannot
supply facts that only arrive with one — a `serial` from a POSTed body, a `mac` from a
query string. A template needing those is reported as a problem:

```
FAIL group "rack-a" member "98fa9b50d811": template needs {{ serial }}, but this request carries no "serial"
```

That is accurate — `check` genuinely cannot prove that answer renders — but it means a
set that deliberately templates on request facts will not come back clean. Verify those
with `render --query` and representative facts. See
[templating](./templating.md#check-and-request-only-facts).

## In CI

If your answers live in git, this is worth a job of its own:

```yaml
# .github/workflows/answers.yml
name: answers
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Get rescriptum
        run: |
          curl -fsSL https://github.com/z29k/rescriptum/releases/latest/download/rescriptum-x86_64-unknown-linux-musl.tar.gz \
            | tar xz --strip-components=1
      - run: RESCRIPTUM_ANSWERS_DIR=answers ./rescriptum check
```

Add `proxmox-auto-install-assistant` to the runner and the same job schema-checks the
TOML too.

## Before deploying

[`deploy.sh`](../operations/deployment.md#replacing-a-running-instance) runs `check`
before it ships anything, and refuses to deploy if the answers do not come back clean.
Serving a broken answer set is worse than not deploying.
