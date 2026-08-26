# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Every machine boots the same image and asks the same server, so the address inside the image
cannot be what distinguishes them. What distinguishes them is what they say when they ask: a
MAC address, a serial number, a hardware inventory. **rescriptum reads that, picks the
documents that apply, merges them, and sends back one config for that one machine.** It
speaks Proxmox `answer.toml`, Ubuntu autoinstall, kickstart, preseed, Ignition, AutoYaST,
unattend.xml and iPXE scripts.

The name is the Roman-law term for an authority's written answer to a question raised by a
particular case: you described your situation, you received a document drafted for it.
That is the protocol exactly — a machine describes its hardware, and gets back a document
composed for it.

The original French specification is at `plans/rescriptum-spec.md`. It is the record of
what was first asked for, **not a description of what exists**: the project has since
outgrown it in every direction — multi-OS, selectors, templating, an admin API, a database
store. Where this file and the spec disagree, this file is right. `plans/` is gitignored,
so a contributor will not have it.

## Deployment reality

Two very different targets, and the design has to satisfy both:

- **A Synology DS416j** (ARMv7, 512 MB, DSM 7, no Docker) — the original motivation. Hence a
  static, tiny, runtime-free binary.
- **Datacenters**, where the server fields thousands of requests and the answers directory holds
  a file per machine. Hence async, bounded concurrency, and the cached listing.

Neither is hypothetical, and a change that suits one at the other's expense is not a win.

## How a request arrives

Since PVE 8.2 the Proxmox installer **POSTs** a JSON body describing detected hardware
(NICs, MACs, disks, DMI) and expects the answer as the response body. That is why a static
file server cannot do this — the reply depends on the request.

Every other installer **GETs**, with the machine's identity in the query string, because
iPXE substitutes it into the URL it was told to fetch. Both are answered on any path; see
*Endpoints, formats* for how the URL decides what may answer.

## Hard constraints

These are deliberate design decisions, not oversights. Do not "improve" them without asking:

- **Async, on tokio + hyper.** The spec said zero dependencies and a thread per connection;
  both were overridden deliberately once the requirement became "absorb a professional
  provisioning burst". Direct deps: `tokio`, `hyper` (server + http1), `hyper-util`,
  `http-body-util`, plus `toml_edit`, `serde_json`, `serde_yaml_ng` and `quick-xml` for
  answer documents — 64 crates, 2.4 MB static on armv7 (1.3 MB without SQLite). Still **no
  `serde` derive anywhere**.
- **hyper directly, not axum.** axum gives no way to set a header-read timeout, which is
  precisely the slowloris guard that motivated going async. Routing here is one `if` on method
  and path, so a framework buys nothing.
- **The request body is parsed as JSON only to harvest facts, and only as an untyped
  `serde_json::Value`.** The spec's "never parse the request JSON" was relaxed deliberately
  when selecting on a serial became necessary — it cannot be reached any other way, because
  the URL baked into an ISO is the same for every machine. What still holds: no struct is
  derived, so no assumption about Proxmox's schema is baked into a type; a body that is not
  JSON is not an error, it just contributes the haystack. Going further — deriving types for
  the body — is a discussion, not a unilateral change.
- **Bounded concurrency, even though tasks are cheap.** A connection costs kilobytes, not a
  thread — that is the whole point of the async rewrite — but unbounded accept still turns a
  burst into an out-of-memory. A `Semaphore` of `RESCRIPTUM_MAX_CONNECTIONS` caps in-flight
  connections; over the cap, the server writes a prompt 503 and closes rather than queueing.
- **Filesystem work goes through `spawn_blocking`.** `read_dir` and `read` are blocking calls,
  and blocking an async worker thread stalls every other connection that thread is driving. On
  a NAS with a sleeping disk that is not theoretical. `resolve()` in `main.rs` holds both, and
  is only ever called inside `spawn_blocking`. A panic there returns a 500; it cannot take the
  server down.
- **Never panic on malformed input.** Any parse failure becomes an error response plus a log
  line. Write the code as if there were no safety net — but there is one, deliberately: the
  release profile does **not** set `panic = "abort"` (it departs from the spec here). With one
  thread per connection, unwinding contains a panic to the connection that caused it instead of
  killing a server mid-install. Measured cost on armv7: +2416 bytes, +0.8%. Do not "optimize"
  it back. If the design ever moves to a thread pool, add `catch_unwind` at the worker boundary
  — a pool thread that dies silently is worse than either.

## Layout

- `src/select.rs` — normalization + answer-file selection. Pure logic, heavily unit-tested;
  this is where the behaviour that matters lives.
- `src/lib.rs` — the crate. `main.rs` is a thin binary over it, so behaviour can be tested
  directly. **Never re-declare a module in `main.rs`**: it compiles a second copy, runs every
  unit test twice, and lets the two copies drift.
- `src/store/` — where documents come from. `mod.rs` defines the thin `Store` / `StoreWrite`
  traits, `file.rs` a flat directory of documents, `sqlite.rs` a bundled-SQLite database.
- `src/facts.rs` — what a request says about the machine: query parameters, a flattened
  JSON body, and the raw haystack.
- `src/format/` — one interface per document format. `xml.rs` holds the XML tree and its
  merge rules.
- `src/merge.rs` — the TOML merge, used by `format`.
- `src/cli.rs` — the `render`, `check`, `import` and `export` subcommands.
- `src/admin.rs` — the write API: its own listener, the constant-time token, the failure
  guard, and the rollback that keeps a write from breaking the answer set.
- `src/capture.rs` — recording request bodies (`RESCRIPTUM_CAPTURE_DIR`).
- `src/config.rs` — environment configuration. `Config::from_lookup` takes a lookup closure so
  tests never touch the process environment.
- `src/envfile.rs` — the optional file of defaults `RESCRIPTUM_ENV_FILE` names. Never
  discovered, only named (this runs as root; a `./.env` would hand the admin token to
  whoever could write in the working directory), the real environment wins over it, and a
  file that was asked for and cannot be read is a startup error.
- `src/log.rs` — one line per event, UTC timestamps computed without a date crate, and the
  two knobs over it. `problems` filters on the status `request()` is handed, so a new call
  site passing the wrong one silently changes what is visible; `0` means "never reached a
  status" and counts as a problem. A failed write is dropped, never propagated — a server
  that died because its log disk filled would fail every install in flight to report that
  it could not report something.
- `src/main.rs` — runtime setup, accept loop, connection serving, routing, logging, and the
  blocking `resolve()` half of a request.
- `tests/integration.rs` — starts the real binary on an ephemeral port (it prints the address
  it actually bound, so there is no port race) and talks HTTP to it. It keeps the server's
  stderr, which is what makes a startup warning assertable.
- `tests/stores.rs` — every behavioural case, run once per store, asserting the identical
  outcome. **A new behaviour belongs here**, not in a store-specific test.
- `tests/admin.rs` / `tests/guards.rs` — the admin API end to end, and the answer token with
  the lockout that deliberately is not there.
- `tests/cli.rs` — `render` / `check` / `import` / `export` against the real binary. `check`'s
  **exit code** is what `deploy.sh` keys on, and `render`'s stdout/stderr split is what makes
  `render … > answer.toml` work; both are contracts, not conveniences.
- `docs/` — the documentation site (see *Documentation* below). Nothing in `src/` knows it
  exists.

## Selecting a machine

Two ways to claim a request, ordered by how narrowly they target one machine:

1. **By identity** — a document named after the machine, or a group listing it in
   `members`. Naming a machine is as specific as it gets, so it always wins
   (`IDENTITY_SCORE` in `select.rs`).
2. **By selector** — a `match` block tested against the request's facts. Among
   selectors, the one satisfying more criteria wins.

Ties break on sorted name. matchbox, the closest prior art, documents that its own
resolution between competing groups "will not be deterministic"; this one is, and a test
pins it.

`facts.rs` turns a request into labelled values from three sources:

- **Query parameters** (`?mac=…&uuid=…&serial=…`) — how every installer other than
  Proxmox identifies itself, since iPXE substitutes them into the URL. Their values also
  feed the haystack, so a document named after a MAC resolves whether that MAC arrived in
  a body or a query string.
- **A JSON body**, flattened to both full paths (`dmi.system.serial`) and bare leaf names
  (`serial`). **The leaf form is the point**: Proxmox documents that the contents of
  `dmi` "might vary wildly, depending on the system", so a selector written against a
  leaf name survives a reorganisation that a fixed path would not. This is the deliberate
  departure from the spec's "do not parse the JSON" — a serial cannot be reached any
  other way, because the URL baked into an ISO is the same for every machine.
- **The raw body**, normalized, as the original substring haystack.

Patterns support `*` and `?`. Both sides are normalized first, so separator style and
case never matter — but note `normalize_pattern`, which keeps the glob characters that
the ordinary normalization would strip.

## Endpoints, formats, and why they are the same thing

An installer fetching a URL expects one particular thing back — a kickstart client wants
kickstart. That is the protocol, not a convention anyone chose. So:

- **the endpoint declares the format** (`format::endpoint_formats`, a small alias table:
  `proxmox`→toml, `rhel`→ks, `ubuntu`→yaml, …);
- **the document carries it as its extension**;
- **only documents of that format may answer.**

This is deliberately **not** tied to storage. Directories and database rows are a lookup
space that must stay free to be reorganised; a URL is a public contract baked into an ISO
and must not move because someone renamed a folder. An earlier design made the directory
name *be* the URL segment and was discarded for exactly that reason.

The consequence that makes the model click: **a machine's answer is specific to the OS it
is for**, so `98fa9b50d810.toml` is not "that machine" but "that machine as Proxmox".
`98fa9b50d810.preseed` is the same hardware as Debian, and both exist at once. The store's
key is therefore **(id, format)**, not id — which is what the SQLite schema is built around.

Two traps in the alias table:

- **Filter on the extension, not the `Kind`.** `.ks` and `.preseed` are both `Kind::Text`;
  filtering by family would let a preseed answer `/rhel/ks`.
- **An alias must be specific enough that nobody reaches it by accident.** `seed` was
  removed: `s=http://server/seed/` is an ordinary NoCloud seed URL, and it serves YAML.

A segment naming no alias constrains nothing, so `/answer` keeps working.

## Templating

Values may carry `{{ key }}`, filled from the request's facts plus `machine` and `group`
(which the facts cannot carry — they are only known once matching has happened). One
group then covers a rack instead of one file per machine.

Two rules that are load-bearing:

- **Substitution happens on parsed string values, never on raw document text**, so the
  format's own serializer escapes the result. A value containing a quote cannot break the
  TOML it lands in. A test feeds `a"b'c<d>e&f` into all four structured formats and
  reparses the output.
- **A missing fact is an error, never an empty string.** Serving `node-.example.com`
  installs a machine with a broken hostname and nobody notices until later. Control
  characters are refused too: a newline in a kickstart value injects a directive.

`Group::has_placeholders` is why a group with no template still costs no parsing per
request — the string prepared at load is served as-is.

## Formats

Determined by file extension, from an allowlist in `format/mod.rs` — `txt` is
deliberately **not** on it, so a stray notes file never becomes a candidate.

| Extension | Layering |
|---|---|
| `toml`, `yaml`/`yml`, `json`/`ign`, `xml` | structural merge |
| `ks`, `cfg`, `preseed`, `seed`, `ipxe` | concatenation in layer order |

Maps merge key by key; **arrays replace rather than append**, so a list can still be
shortened from a higher layer. Text formats are *concatenated*, and the module says so
rather than pretending otherwise — whether that amounts to an override is the target
format's business.

XML pairs siblings by name plus a discriminating attribute (`name`, `id`, `key`, `alias`,
`pass`), which is what makes `<component name="Shell-Setup">` and `<settings
pass="specialize">` in an unattend.xml mergeable, and honours AutoYaST's
`config:type="list"`. Declarations, doctypes, namespaces and attributes survive; original
indentation and comment placement do not, because the output is re-rendered rather than
patched. It understands no schema; render and check before trusting a rack to it.

`examples/` carries a worked example of **all thirteen extensions** — its `README.md` is the
map — and `RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check` exercises them all.
**Keep it that way, and keep `check` green there**: `deploy.sh` refuses to ship when it is
not, the examples are the only place the formats are shown composing together, and two of
them (`suse-node.autoyast`, `windows-node.unattend`) are what caught the missing doctype
and the unpaired `pass`. Note the directory is named `examples/`, not `answers/`: these are
documentation, they carry `REPLACE$ME` credentials, and nothing should ever be served from
them.

Control keys travel in whatever the format allows: native top-level keys in the
structured ones, an `<answer-meta>` element in XML, `# answer:` directives in text. All
are stripped before the answer is sent. **Every layer of one answer must be the same
format** — a YAML machine file over a TOML group is refused, not half-served.

## Grouping and merging

A datacenter has a file per machine, and machines in a rack share almost everything. Answer
files therefore compose:

```text
answers/
  groups/
    base.toml            shared by everything
    rack-a.toml          extends = "base"; members = [ ...MACs... ]
  98-fa-9b-50-d8-10.toml one machine's overrides (optional)
  default.toml           only when nothing else matches
```

- A **group** claims machines by listing them in `members`. Member strings are normalized the
  same way filenames are, so separator style does not matter.
- A group may `extends` another group, giving a chain. Cycles and missing parents are detected
  at load, reported once, and the broken group is dropped rather than half-applied.
- A **machine file** layers on top of whichever group claimed it. `extends` in a machine file
  overrides membership — the escape hatch for a machine that needs a group it is not listed in.
- Layers apply lowest to highest: group chain, then machine file. **The machine always wins.**
- `extends` and `members` are ours, not Proxmox's, and are stripped before the answer is sent.

Merge rules (in `merge.rs`): tables merge recursively, including inline and dotted tables; any
other value is replaced outright; **arrays replace rather than concatenate**, because appending
would make a list impossible to shorten from a higher layer.

Only the first matching group applies — matching is by sorted name. Composition is expressed
with `extends`, not by merging every group that happens to match, because ordering between
several matching groups would be arbitrary.

Performance, measured at 2,000 machines (3000 requests, 100 concurrent):

| Layout | Throughput |
|---|---|
| 2,000 machine files, no group | 12,132 req/s |
| one group of 2,000 members, no machine files | 13,036 req/s |
| 2,000 machine files plus a group (merge per request) | 8,816 req/s |

Grouping is the *fast* path: a group with no machine overrides is rendered once at load and
served as a prepared string, so the common datacenter case parses nothing per request.

### The validation gap this creates

Before merging, an admin wrote a complete file and validated it with
`proxmox-auto-install-assistant validate-answer`. A composed answer is something nobody has
ever seen, and a bad merge surfaces as a failed unattended install. Two subcommands exist for
exactly this, and any change to merging must keep them working:

```bash
rescriptum render 98:fa:9b:50:d8:10   # print what that machine would receive
rescriptum render --body captured.json
rescriptum check                      # render everything, report what breaks
```

`check` proves answers are well-formed and merge cleanly; it does **not** know the Proxmox
schema. Piping `render` into `validate-answer` is what does.

## Two stores, one behaviour

Answers come from either a flat directory of documents (`RESCRIPTUM_STORE=files`, the default) or a
SQLite database (`RESCRIPTUM_STORE=sqlite`), chosen at runtime.

The store is deliberately **thin**: it hands back raw TOML text and a cheap `version` token,
and nothing else. Every decision — matching, `extends` chains, merging, rendering, `check` —
lives above it in `select.rs` and `merge.rs` and is shared. Keep it that way: the moment a
backend starts deciding behaviour, the two drift.

`tests/stores.rs` is the guarantee. Every behavioural case runs twice, once per store, and
asserts the identical outcome. **A new behaviour belongs there, not in a store-specific test.**

Both are write-capable (`StoreWrite`), which is what the admin API will use:

- **Files** write via a temporary file plus `rename`, atomic on POSIX, so a reader never meets
  a half-written answer. A test asserts no `.tmp` file survives.
- **SQLite** is compiled in (`rusqlite`'s `bundled` feature — it cross-compiles to armv7-musl
  under zigbuild, verified). WAL mode, so the admin API never stalls an install. `version()`
  reads an in-process atomic rather than querying, because it is called per request; a change
  made by another process is caught by `RELOAD_BACKSTOP` instead, and a test proves it.
  Opening a database whose `user_version` is newer than this binary understands is refused
  rather than guessed at. **There is one schema version and `migrate()` has no steps**:
  nothing has been released, so the shapes it went through while being written never left
  the repository, and migrations from them would be code that cannot run. Adding a second
  version means adding its step there, guarded by `if current < 2`.

`import <dir>` and `export <dir>` move between the two. The round trip is byte-identical, which
is worth keeping true — it is what makes the database safe to adopt and safe to leave.

The `sqlite` cargo feature is on by default and can be turned off: 2,103,456 bytes with it,
944,928 without, on armv7.

## The admin API

`src/admin.rs`, enabled only by `RESCRIPTUM_ADMIN_ADDR`, and only over SQLite. Three properties
are load-bearing — a change that quietly drops any of them is a regression:

1. **Its own listener.** The answer endpoint is unauthenticated by necessity (the installer
   has no credentials). This API sets the root password and SSH keys of every machine
   installed afterwards. It never shares that port. `Config::validate` refuses to start
   without a token, with a token under 16 characters, or over the file store — as startup
   errors, not warnings.
2. **SQLite only.** Over files there would be two ways to change the same configuration,
   by hand and over the wire, racing each other.
3. **A write can never leave the answer set broken.** `guarded()` snapshots `problems()`,
   applies the write, and compares. Anything newly broken is rolled back and answered
   `409`. This is why a machine's `extends` pointing at a missing group is detected at
   *load* time in `select.rs` rather than only when that machine asks: the guard can only
   catch what `problems()` reports.

Auth is a bearer token compared in constant time — an ordinary `==` returns early on the
first differing byte, which leaks the token one byte at a time to anyone timing the
responses. `AuthGuard` shuts out an address after `MAX_FAILURES` within `FAILURE_WINDOW`,
backing off exponentially to `MAX_BLOCK`, and the block deliberately applies to a *correct*
token too — otherwise guessing until you got it right would cost nothing. It is per-address
and bounded to `MAX_TRACKED` entries, so the guard cannot itself be turned into a memory
leak. `GET /health` stays unauthenticated and unblocked, so monitoring does not go dark
during an attack.

Known and accepted: per-address limiting does not stop an attacker with many addresses, and
the API speaks plain HTTP (put TLS in front if it leaves loopback). The token's length is
what actually makes guessing hopeless — hence the 16-character floor at startup.

## Two guards, and why they differ

- **`RESCRIPTUM_ANSWER_TOKEN`** protects the answer endpoint. Proxmox sends one when its ISO was
  prepared with `--answer-auth-token`. Off by default and necessarily so — most installers
  have no credential. Failures are logged but **never rate-limited**: a rack can sit
  behind one address, and shutting it out turns a bad token into a failed rollout. The
  admin API, which no installer talks to, does lock out. A short token there warns rather
  than refusing to start, because refusing would leave a fleet unable to install.
- **`RESCRIPTUM_CAPTURE_DIR`** records what machines actually send, `.body` verbatim plus a
  `.meta`. This exists because everything here was built against documentation; until a
  real installer has talked to it, that is a claim. Capped at `MAX_CAPTURES`, never deletes,
  and a capture failure is logged rather than costing an install.

`check` calls the installer's own validator when it is on PATH, and names the formats it
could not check. Note it needs `Resolution::format_name` (the extension), not
`format.label()` (the family): `ks` and `preseed` share a family but not a validator.

## Traps already hit (do not re-discover these)

- **hyper panics if a timeout is set without a timer.** `http1::Builder::header_read_timeout`
  requires `.timer(TokioTimer::new())`. Omit it and every connection panics at runtime — it
  does not fail to compile. The integration tests caught this; unit tests could not have.
- **`header_read_timeout` stops at the end of the headers.** hyper has no body-read timeout, so
  a client that promises a body and sends nothing would park a connection indefinitely. The
  whole-connection `tokio::time::timeout` in `connection()` is what covers that; both are
  needed, neither is redundant.
- **An aberrant `Content-Length` is refused before the body is read at all**, from the header,
  rather than by letting `Limited` trip after buffering a megabyte.
- **Closing on a peer that is still writing discards the response you just wrote.** The kernel
  sends a reset and the reset throws away the unread bytes. `shed()` had this: it wrote its
  `503` and closed, so the installer it was telling to retry got a connection reset instead.
  It drains for `SHED_DRAIN` first now, the way `admin::put()` already did. Same trap, two
  places — check any new "answer then close" path for it.
- **hyper emits header names lowercased.** That is correct (they are case-insensitive), so
  assert on a lowercased copy — see `has_header` in the integration tests.
- **`fs::metadata` per directory entry is a stat syscall each.** `DirEntry::file_type()` comes
  back free with the readdir on Unix; only a symlink needs the stat to resolve. That alone was
  worth 65% at 2,000 files, before caching.
- **Cache-invalidation tests must share one `Answers` instance.** A test that constructs a fresh
  one per call bypasses the cache entirely and silently proves nothing.
- **A new test has to be watched failing.** Break the thing it guards and check it goes red
  before trusting it. One here claimed to protect the `version.is_some()` clause in the
  listing cache and stayed green without it — with either store a version is unreadable only
  when the store is also empty, so that clause cannot currently fire. A test that passes for
  the wrong reason reports coverage that does not exist.
- **Answer files must now be valid TOML.** Before merging they were served as opaque bytes, so a
  malformed file reached the installer; now it is a 500 with the parse error in the log. That is
  the better failure, but it is a behaviour change — fixtures written as YAML-ish text stopped
  working.
- **Editing a group file's *contents* changes no directory mtime.** Only `RELOAD_BACKSTOP`
  (1 s) picks that up, which is why the backstop is not redundant with the mtime check. An
  integration test covers it.
- **A `python`/`sed` patch that "succeeds" may have matched nothing.** Two edits in this
  project's history silently no-opped and were only caught by checking test counts afterwards.
  Assert the old text was found before writing.
- **Admin responses must set `Connection: close`.** Without it every test client waited
  out the connection timeout — the suite took 30 s instead of 0.4 s — and the eventual
  drop sometimes arrived as a reset rather than a clean EOF.
- **Read the request body before rejecting a request.** Answering and closing while the
  client is still writing earns it a `ECONNRESET` instead of the response. `put()` drains
  first, then validates the identifier.
- **Identifiers become filenames.** `export` and the file store build paths from machine
  ids and group names, so `valid_id` is enforced at the API boundary *and* in both stores.
- **A GET has no body, so the haystack is empty.** Query values must feed it too, or a
  document named after a MAC can never answer a preseed or kickstart fetch.
- **Normalizing a selector pattern strips `*` and `?`** unless you use
  `normalize_pattern` — which turns every glob into a literal, quietly.
- **In a text format, a placeholder inside a comment is still a placeholder.** `Kind::Text`
  is an opaque string, so substitution runs over the whole document: a `{{ mac }}` written
  in a `#` comment to *explain* templating still has to resolve, and fails `check` like a
  real one.
- **quick-xml emits entity references as their own events.** Ignoring them welds the
  surrounding text fragments together: `1 &lt; 2 &amp; 3` came back as `123`.
- **Repeated XML siblings are not always a list.** If they carry a discriminating
  attribute they are a keyed collection; treating them as a list replaced every
  `<component>` in an unattend.xml with the one the overlay happened to mention.
- **`cargo test` does not rebuild `target/debug/rescriptum`.** A manual check
  against a stale binary "reproduced" a bug that had already been fixed. Rebuild before
  poking at the binary by hand.
- **Two documents with the same stem are not duplicates.** An earlier `put` deleted the
  other formats of a stem to avoid "two answers for one machine". That was the wrong
  model: they are that machine's answers for two operating systems.
- **Assert on parsed values, not on formatting.** Replacing a table with a scalar leaves the
  key's original decor, so the output can read `value= 3` — valid TOML, different text.

## Core algorithm (the part worth understanding up front)

Answer files live in a configurable directory, named after a MAC address
(`98-fa-9b-50-d8-10.toml`, `aabbccddeeff.toml`, plus an optional `default.toml`).

Selection per request:

1. Normalize the request body: lowercase, strip every non-alphanumeric character.
2. For each `<stem>.toml` in the directory (excluding `default.toml`), normalize `<stem>` the
   same way and test whether it appears as a substring of the normalized body.
3. First match wins → return that file.
4. No match → `default.toml` if present, else `404`.

The normalization is what makes this robust: it's indifferent to MAC separator style
(`98-fa-9b…` / `98:fa:9b…` / `98fa9b…`) and to the JSON structure, which changes between Proxmox
versions.

The listing is **cached and invalidated by the directory's mtime**, which changes whenever a
file is added or removed. The spec asks for a re-read on every request; done literally that is a
`readdir` plus a sort plus a normalization pass per request, and with one answer file per
machine — the datacenter case — throughput collapses. Measured, 3000 requests at 100 concurrent:

| Files in the directory | Literal re-read | mtime-cached |
|---|---|---|
| 10 | 11,954 req/s | 12,922 req/s |
| 200 | 3,198 req/s | 12,890 req/s |
| 2,000 | 311 req/s | 12,520 req/s |
| 10,000 | — | 6,924 req/s |

One `stat` replaces the whole walk, and a new file is still picked up with no restart — which is
the guarantee the spec actually wanted. `RELOAD_BACKSTOP` (1 s) forces a re-read even when mtime
looks unchanged, covering filesystems with coarse mtime granularity. Normalized stems are
computed once per directory read, not once per request.

The remaining cost at 10,000 files is the linear scan of precomputed needles — pure CPU, no
syscalls. Bucketing needles by length and sliding a window over the body would remove it, but a
10,000-machine rollout already completes in under two seconds, so it has not been worth the
complexity. Measure before adding it.

## HTTP surface

- Accept `POST` on **any path** — the URL is baked into the ISO, so don't impose a fixed route.
- `GET /health` → `200 OK`. Every other method → `405`.
- Success: `200`, raw TOML body, `Content-Type: text/plain`, correct `Content-Length`,
  `Connection: close`.
- `404` when no file applies; `500` on read errors.
- Reject an implausible `Content-Length` (cap at 1 MB) rather than allocating for it.
- Only read direct entries of the answers directory. Never build a filesystem path from
  request data — that is the path-traversal guard.

Logging goes to stdout/stderr, one line per request (timestamp, source IP, body size, chosen
file or failure). When a PXE install won't start, this is the only diagnostic available.

## Configuration

Environment variables only — plus an optional file to read some of them from:

| Variable | Default | Role |
|---|---|---|
| `RESCRIPTUM_ENV_FILE` | unset | A file of the same variables. See below |
| `RESCRIPTUM_STORE` | `files` | `files` or `sqlite` |
| `RESCRIPTUM_ANSWERS_DIR` | `/srv/answers` | Directory of answer documents |
| `RESCRIPTUM_DB_PATH` | `/srv/answers.db` | SQLite database, when `RESCRIPTUM_STORE=sqlite` |
| `RESCRIPTUM_LISTEN_ADDR` | `0.0.0.0:8000` | Listen address (`:0` picks a free port) |
| `RESCRIPTUM_WORKERS` | CPU count | tokio runtime threads (not a concurrency limit) |
| `RESCRIPTUM_MAX_CONNECTIONS` | `2048` | In-flight connections before shedding with 503 |
| `RESCRIPTUM_TIMEOUT_SECS` | `10` | Header-read timeout **and** whole-connection deadline |
| `RESCRIPTUM_LOG` | `all` | `all` \| `problems` (drops the requests that worked) \| `off` |
| `RESCRIPTUM_LOG_FILE` | unset | A file to append to, or `stdout`/`stderr`. Unopenable is fatal |

A zero or unparseable numeric value falls back to the default rather than starting a server
that accepts and never answers.

**What is fatal and what is a warning is deliberate.** Fatal: the listener cannot bind, the
store cannot be opened (SQLite catches an unwritable directory, a corrupt file and a
too-new schema at open, not at the first request), a named env or log file cannot be read,
and any unsafe admin combination. A warning: the answers directory is absent, is not a
directory, or cannot be listed, and any problem in the answer set — all three can be fixed
while the server runs, and it re-reads as they change. The directory check asks the
filesystem whether it can list rather than reading permission bits, so it accounts for
owner, group, ACLs and the mount; that is the failure a packaged, non-root run meets first.

**`RESCRIPTUM_ENV_FILE` (`src/envfile.rs`)** exists for DSM 7, which has no systemd: its
Task Scheduler entry otherwise has to `. file && exec …`, and that fails *silently* — drop
the `.` and the server comes up on its defaults with no admin token and nothing in the log.
Three properties are the point, and a change that drops one is a regression:

- **Never discovered, only named.** There is no `./.env`. This runs as root; a file picked
  up from the working directory would hand `RESCRIPTUM_ADMIN_TOKEN` to whoever could write
  there.
- **The real environment wins**; the file is defaults. An exported-but-empty variable
  counts as unset, so the file still applies.
- **Unreadable or malformed is a startup error**, never a warning — that is the silent path
  being removed. A *misspelled* key and a group-readable file are warnings (naming keys and
  paths, never values).

It is not a shell: no `${}` expansion, no inline comments (a `#` in a value is part of the
value — truncating a token silently is worse than a comment landing in a value, where it is
loud), `export` accepted so one file can also be sourced, a duplicate key is an error.

## Commands

Local development (once the crate exists):

```bash
cargo build
cargo run -- check               # validate an answers directory
cargo run -- render <mac>        # print one machine's composed answer
cargo test                       # all tests
cargo test <name>                # single test by name substring
cargo test -- --nocapture        # show stdout from tests
cargo fmt && cargo clippy
```

Documentation (see *Documentation* below):

```bash
npm install                      # once — the docs toolchain only
npm run docs                     # review server at http://localhost:3009
npm run docs:build               # the public artifact into ./_site
npm run docs:lint                # no broken internal links (CI gate)
```

Cross-compile for the NAS (ARMv7 hard-float, static musl):

```bash
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

`cargo-zigbuild` uses Zig as the linker, avoiding a full cross toolchain. The toolchain is
**installed and verified**: Rust 1.93, targets `aarch64-apple-darwin` +
`armv7-unknown-linux-musleabihf`, `cargo-zigbuild` 0.23.0, Zig 0.16.0. A scratch crate built
with the release profile below produced `ELF 32-bit LSB executable, ARM, EABI5, statically
linked, stripped` at 296 KB — so the chain works end to end.

Zig is **not** a Homebrew install: `brew install` aborts on this machine over untrusted
third-party taps (`shivammathur/php` and friends), unrelated to Zig. It was installed from the
official tarball into `~/.local/zig`, symlinked at `~/.local/bin/zig` (already on `PATH`). To
upgrade, replace that directory — don't assume `brew upgrade zig` does anything.

The spec calls for `build.sh` (builds and prints the binary size) and `deploy.sh` (build, copy,
restart the DSM task) as deliverables.

Verify the first cross-build actually produces a static binary (`file` should say *statically
linked*) and that it runs on the NAS. If ARMv7 misbehaves, confirm the real architecture with
`uname -m` on the NAS before pushing further.

## Release profile

`Cargo.toml` optimizes for size, minus the spec's `panic = "abort"` (see Hard constraints):

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
```

## Testing expectations

309 tests. `docs/development/testing.md` has the per-suite table; the rules that decide where
a test goes:

- **A behaviour belongs in `tests/stores.rs`**, which runs it against both stores and requires
  the identical outcome. One that covers a single backend proves half of what it claims, and
  the half it skips is where a divergence hides.
- **Anything about the wiring belongs in `tests/integration.rs`**, against the real binary over
  a real socket. Some failures are invisible to unit tests: hyper *panics at runtime* if
  `header_read_timeout` is set without a timer, and it compiles. Abuse cases (truncated
  request, no `Content-Length`, chunked body over the cap, non-UTF-8 body, the connection cap)
  each end by proving the server still answers — that last assertion is the one that matters.
- **The commands people are told to run belong in `tests/cli.rs`.** `check`'s exit code is what
  `deploy.sh` keys on and what CI runs; `render`'s stdout/stderr split is what makes
  `render … > answer.toml` work. Both are contracts.
- **Watch a new test fail before trusting it.** Break the thing it guards. One here claimed to
  protect the `version.is_some()` clause in the listing cache and stayed green without it,
  because with either store a version is unreadable only when the store is also empty. A test
  that passes for the wrong reason reports coverage that does not exist.
- **Assert on parsed values, not formatting**; **cache-invalidation tests must share one
  `Answers`** or they bypass the cache and prove nothing; **`Config::from_lookup` takes a
  closure** so configuration tests never touch the process environment.

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

## Language

Code, comments, specs, commit messages, and the **source** of the documentation are written in
**English**. (Conversation with the maintainer happens in French — that does not change what
gets written to disk.)

The one deliberate exception: **the documentation is published in English *and* French.**
English is the source; every page under `docs/` has a `*.fr.md` sibling, and `README.md` has
`README.fr.md`. Write the English first, then translate — an unmirrored English change leaves
the French page stale rather than broken. Details in `docs/development/docs-site.md`.

## Branching model and releases

Mirrors the conventions of the sibling project `notabene` (`~/Dev/z29k/notabene`, see its
`CONTRIBUTING.md`), deliberately — same maintainer, same expectations:

- **`main`** — stable. Only release commits and `vX.Y.Z` tags land here; never push feature
  work directly.
- **`develop`** — integration. Kept at the in-progress next version.
- **`feature/<name>`** / **`fix/<name>`** — branch from `develop`, PR back into `develop`.
  When `develop` is ready, bump the version, promote to `main`, tag `vX.Y.Z`.

Conventional commits with a scope (`feat(http): …`, `fix(select): …`, `chore: release vX.Y.Z`).
SemVer tags. Keep PRs focused.

What does **not** carry over from notabene: it is an npm package and publishes prereleases to
an `@dev` dist-tag. This project ships a **compiled binary**, so the release artifact is a
GitHub Release with cross-compiled binaries attached, built by a CI matrix. A DSM community
package may follow later.

CI gates on every push: **gates** (fmt, clippy, tests, the no-SQLite build), **docs** (public
build plus `notabene lint`), **audit** (`cargo audit --deny warnings`, which fails on an
unmaintained or yanked crate as well as a vulnerability — an unfixable one gets
`--ignore RUSTSEC-…` with a reason, not the flag removed), and **cross** (ARMv7-musl, then
asserting the binary is statically linked). Every action is an official `actions/*`; Zig and
`cargo-audit` are installed directly, because this toolchain vets and links a binary people
run as root.

**`develop` publishes nothing.** It runs the CI gates (build, tests, clippy, fmt) and stops
there — no prereleases, no artifacts. Binaries are produced only by a `vX.Y.Z` tag on `main`.

Release target matrix (settled):

| Target | For |
|---|---|
| `armv7-unknown-linux-musleabihf` | the DS416j, the reason this project exists |
| `aarch64-unknown-linux-musl` | modern ARM NAS / Raspberry Pi |
| `x86_64-unknown-linux-musl` | most other Linux hosts |
| `aarch64-apple-darwin` | local development |
| `x86_64-apple-darwin` | local development |

## Project conventions

This is intended as a public open-source GitHub project (remote to be added once a V1 is
acceptable), serving both personal and professional use — so treat robustness, release
management across branches, and documentation quality as requirements rather than polish.

Deliverables per the spec: `src/main.rs` (split into modules if size warrants), `Cargo.toml`,
a commented `examples/example.toml` covering `global` / `network` / `disk-setup`, `build.sh`,
`deploy.sh`, a Rust `.gitignore`, and a license. The spec's "README covering purpose,
cross-compilation, DSM deployment, ISO preparation and troubleshooting" is now `docs/`
instead — the README had grown to 28 KB and was three documents wearing one coat. It is a
landing page linking into the site.

Out of scope but documented rather than implemented: TLS (plain HTTP is fine on a
trusted LAN — document the workaround for installer versions demanding a cert fingerprint) and
URL discovery via DNS TXT / DHCP option 250.

Prior art worth consulting: <https://github.com/SlothCroissant/proxmox-auto-installer-server>
Proxmox reference: <https://pve.proxmox.com/wiki/Automated_Installation>
