---
title: Templating
description: Placeholders filled from the request, so one group file covers five hundred machines — and why a missing value is an error rather than an empty string.
sidebar:
  label: Templating
  order: 4
---

# Templating

Grouping removes the duplication between machines that agree. Templating removes the last
reason to write a file per machine at all: the values that must differ.

```toml
# answers/groups/rack-a.toml
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11", "…"]

[global]
fqdn = "node-{{ serial }}.example.com"

[network]
filter.ID_NET_NAME_MAC = "*{{ mac }}"
```

Five hundred machines, one file. Without this, a per-machine hostname means a document per
machine — and five hundred documents that differ in one line each.

Placeholders work in **every** format: TOML, YAML, JSON, XML, kickstart, preseed.

> In the structured formats, substitution happens on parsed string values — so a comment is
> just a comment. In the **line-oriented** ones (`ks`, `preseed`, `cfg`, `ipxe`, `seed`) the
> document is an opaque string, so **a placeholder written inside a comment is still a
> placeholder** and still has to resolve. Mentioning `{{ serial }}` in a `#` line to explain
> it to the next reader will fail the render just as a real one would.

## What you can put in one

| Placeholder | Filled from |
|---|---|
| `{{ mac }}`, `{{ serial }}`, `{{ uuid }}`, … | any [fact](./selection.md#the-facts-a-selector-can-test) the request carries — query parameters, and the fields of a POSTed JSON body by leaf name or full path |
| `{{ dmi.system.serial }}` | the same body, by its exact path |
| `{{ path }}`, `{{ file }}`, `{{ segment }}` | the URL the request arrived on |
| `{{ group }}` | the name of the group that applied |
| `{{ machine }}` | the identifier of the **machine document** that matched |

Whitespace inside the braces is optional: `{{serial}}` and `{{ serial }}` are the same.

### `machine` needs a machine document

`{{ machine }}` is the identifier of the machine *document* that matched — so it is only
available when the machine has a document of its own. A machine claimed by a group's
`members` list, with no file next to it, has no `machine` value and rendering fails with
`template needs {{ machine }}, but this request carries no "machine"`.

In a group, use a request fact instead:

```toml
[global]
fqdn = "node-{{ mac }}.example.com"      # works for every member
```

`{{ machine }}` is for a machine document that wants to name itself without repeating its
own MAC.

## A missing value is an error

A placeholder the request cannot fill is a **500 with the reason**, never an empty
string:

```console
$ rescriptum render 98:fa:9b:50:d8:10
error: template needs {{ serial }}, but this request carries no "serial"
```

This is deliberate. Serving `node-.example.com` installs a machine with a broken hostname
and nobody notices until later — possibly much later, on a machine that is already in
production. Failing the install is the cheaper outcome.

Control characters are refused outright for the same class of reason: a newline in a
kickstart value would inject a directive into the file the installer executes.

```console
$ rescriptum render --query "mac=aa:bb&serial=$(printf 'a\nb')"
error: value for "serial" contains a control character and will not be substituted
```

## Substitution is escape-safe

**Substitution happens on parsed values, never on raw document text.** The value is put
into the document's own data model and the format's serializer writes it out — so the
serializer does the escaping.

A serial containing a quote cannot break the TOML it lands in:

```console
$ rescriptum render --query 'mac=aa:bb&serial=a"b'"'"'c<d>e'
[global]
fqdn = """node-a"b'c<d>e.example.com"""
```

The TOML writer reached for a multi-line string on its own. The same value in an XML
document comes back entity-escaped, and in JSON, JSON-escaped. A test feeds
`a"b'c<d>e&f` through all four structured formats and reparses the output.

This is why templating is safe to feed from a request that a machine you have never met
controls.

## `check` and request-only facts

`rescriptum check` renders every machine from its **identity alone** — it has no request
to draw on, because there is no request. A template that needs `serial`, which only ever
arrives in a body or a query string, is therefore reported as a problem:

```console
$ rescriptum check
  FAIL group "rack-a" member "98fa9b50d811": template needs {{ serial }}, but this request carries no "serial"
```

That is honest — `check` genuinely cannot prove that answer renders — but it is noisy for
a set that deliberately templates on request facts. Verify those with `render` and
representative facts instead:

```console
$ rescriptum render --query "mac=98:fa:9b:50:d8:11&serial=7ABC123"
```

## The cost

None, when you are not using it. A group whose prepared string carries no `{{` is served
as-is, without being parsed per request — the check for placeholders happens once, when
the store is read. Templating moves a group onto the merge-per-request path only for the
documents that actually contain one.

## Next

- [Validating](./validating.md) — render with real facts, and check the whole set.
- [Capturing requests](../operations/capture.md) — get a real body to render against.
