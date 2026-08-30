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
  answer documents — 64 crates, 2.4 MB on armv7 (1.3 MB without SQLite). Still **no
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
  traits, `file.rs` a **directory per identity** (see *Layout on disk*), `sqlite.rs` a
  bundled-SQLite database.
- `src/boot/` — **boot media**: where the installer itself comes from, as opposed to what
  it is told. `sources.rs` is the odd one out: it is where images can be fetched *from*,
  and it stores **nothing about any specific image** — each entry names the checksum index
  a vendor already publishes, so the list and the digests are read from them at the moment
  somebody asks. A baked-in table of URLs would ship stale and serve 404s. `iso.rs` reads ISO9660 far enough to turn a path into an offset and a
  length (a file in an image is one contiguous extent, so serving a kernel is a *seek*,
  never an extraction); `probe.rs` places an image from a table of markers; `catalog.rs`
  discovers what is held, cached behind the directory mtime like the answer listing;
  `media.rs` is the listener, on its own socket; `stanza.rs` holds what each installer
  family needs on the wire; `patch.rs` adds one file to an ISO as a *plan* rather than a
  rewrite; `tftp.rs` hands over the loader and nothing else; `loaders.rs` is the option-93
  table **both** TFTP and `boot dhcp-snippet` read, so the two cannot drift; `menu.rs`
  writes the bootstrap and the menu; `dhcp.rs` generates six configuration formats;
  `privileges.rs` drops after binding; `cpio.rs` and `sha256.rs` are hand-written and
  dependency-free. Behind the `boot` cargo feature, default on. `select.rs` knows none of
  this exists, and the only seam is that `media ipxe` **prints an ordinary `.ipxe` answer
  document** — selection, layering and templating then apply unchanged.
- **No installer image is in this repository or in a release.** An ISO is somebody
  else's artefact, gigabytes, on its own schedule. `RESCRIPTUM_MEDIA_DIR` is where a
  deployment keeps them and **that directory is the archive**: `media add <url>` fetches
  one into it (through `curl`/`wget` — there is no TLS in the binary), and nothing ever
  modifies it afterwards. Preparing an image produces a sidecar plus an injection applied
  on the wire, so the bytes on disk stay what the vendor published and stay checkable
  against the vendor's own `SHA256SUMS`. A URL **requires** `--sha256` unless
  `--unverified` is passed: this decides what every machine installs, and the unsafe path
  has to be a deliberate act.
- `packaging/ipxe/` — the branded loaders: `branding.h`, the embedded script, a
  SHA-pinned upstream commit and `build.sh`. **No binaries in git, ever**; this directory
  is the GPLv2 written offer. The built loaders **do** ship — inside the `.spk` and as
  `rescriptum-boot-assets-<version>.tar.gz` — because a TFTP server with nothing to hand
  out boots nothing. That is aggregation, not linking, and the `NOTICE` naming the pinned
  commit travels with them everywhere they go. `packaging/boot-rig/` — the boot rig, three services on an
  `internal: true` network, so a harness that runs DHCP cannot answer on the host's LAN.
- `src/facts.rs` — what a request says about the machine: query parameters, a flattened
  JSON body, and the raw haystack.
- `src/format/` — one interface per document format. `xml.rs` holds the XML tree and its
  merge rules.
- `src/merge.rs` — the TOML merge, used by `format`.
- `src/cli.rs` — the `render`, `check`, `import`, `export`, `migrate` and `config`
  subcommands. `migrate` **shows by default and moves only on `--apply`**, and a single
  taken destination aborts the whole run rather than leaving a half-migrated directory.
  `config` is dispatched **before** `Config::from_env` and `validate`, unlike every other
  one: a file that will not parse and a token one character short are the states people run
  it to get *out* of, so it loads the file itself and reports rather than dying.
- `src/admin.rs` — the write API: its own listener, the constant-time token, the failure
  guard, and the rollback that keeps a write from breaking the answer set.
- `src/capture.rs` — recording request bodies (`RESCRIPTUM_CAPTURE_DIR`).
- `src/installed.rs` — a machine reporting it finished, and its install claim being
  dropped. **The one path where something arriving over the network changes the answer
  set**, so it is narrow by construction: machine documents only (never a group — one
  machine finishing must not disarm a rack), format `ipxe` only (the `.toml` is the record
  of how it was built), and moved into an `installed-<id>/` **sibling directory** rather
  than deleted — a name that identifies nothing, so no new exclusion rule is needed. The
  token is Proxmox's, and it arrives **in the JSON body**, not as a bearer — so the route
  runs before the answer token's guard, which would otherwise reject every webhook. It
  also takes a bearer header and the identity from the query, because **Proxmox is the
  only family with a webhook**: every other one reports from a `%post`, a
  `late_command` or a chroot script, where one `curl` is writable and composing
  Proxmox's JSON is not.
- `src/config.rs` — environment configuration. `Config::from_lookup` takes a lookup closure so
  tests never touch the process environment — which is also the seam every configuration
  file hangs off: a file is just another source behind that closure.
- `src/tomlconfig.rs` — the optional **TOML** file `RESCRIPTUM_CONFIG` names, for the
  platform where a person edits configuration by hand. It **maps a document onto the same
  `RESCRIPTUM_*` names and does nothing else**, so one place still decides what a setting
  means and the file cannot grow behaviour the environment lacks. `MAPPING` is that table,
  and a unit test asserts it covers `envfile::KNOWN_KEYS` exactly — a setting missing from
  it is one the file silently cannot configure. Writes go through `toml_edit`, which edits
  the document in place: **replace the value, never the entry**, because a setting's
  explanation lives in the *key's* decor and inserting over the key throws the paragraph
  away.
- `src/envfile.rs` — the optional file of defaults `RESCRIPTUM_ENV_FILE` names, and the
  writer behind `config set`: `rewrite()` edits lines where they stand, **uncommenting** a
  commented setting rather than appending a duplicate, because on a packaged install those
  comments are the only documentation the configuration has. `write_atomic()` preserves the
  file's **owner** as well as its mode — a root-owned rewrite of a `0600` file the service
  owns is a server that stops starting one restart later. Never
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
- `tests/common/mod.rs` — the one thing every suite shares: `seed()` writes a fixture named
  the way a test thinks of it (`98fa9b50d810.toml`, `groups/rack-a.toml`, `default.toml`)
  **through `StoreWrite`**, so it lands exactly where an admin-API write would and cannot
  drift from the layout. A name the store would refuse is written literally, because those
  fixtures exist to prove a stray file answers nothing. One copy, not one per suite.
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
- `packaging/dsm/` — the Synology package (see *The DSM package* below). Nothing in `src/`
  knows it exists either, and that is the whole design.

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
is for**, so `98fa9b50d810/proxmox.toml` is not "that machine" but "that machine as
Proxmox". `98fa9b50d810/debian.preseed`, beside it, is the same hardware as Debian, and both
exist at once. The store's
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

A datacenter has a directory per machine, and machines in a rack share almost everything.
Answer documents therefore compose:

```text
answers/
  groups/
    base/proxmox.toml            shared by everything
    rack-a/proxmox.toml          extends = "base"; members = [ ...MACs... ]
  98-fa-9b-50-d8-10/proxmox.toml one machine's overrides (optional)
  default/proxmox.toml           only when nothing else matches
```

- A **group** claims machines by listing them in `members`. Member strings are normalized the
  same way directory names are, so separator style does not matter.
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

Two cargo features, both on by default and both removable: `sqlite` and `boot` (the media
catalogue, the ISO reader and the media listener). Measured on armv7-gnueabihf, floor 2.17
— **re-measure rather than trusting an older figure here: the numbers moved by ~375 KB
when the target changed from musl to glibc, and a stale baseline once turned a 71% budget
spend into an apparent 293% overrun.**

| Build | Bytes |
|---|---|
| `sqlite` + `boot` (default) | 2,813,712 |
| `sqlite` only | 2,557,592 |
| `boot` only | 1,649,048 |
| neither | 1,392,544 |

Re-measured 2026-08-29 on armv7-gnueabihf (floor 2.17), all four in one sitting. **Both
tables that held these numbers were stale by roughly 200 KB** — this one and
`docs/guide/reference/configuration`, which disagreed with each other as well.
`boot` costs **1,164,120** against `sqlite` alone by this measurement; the budget question
below is written against the older figure and needs re-deciding against this one.

**`boot` costs 259,360 bytes, against a ≤170 KB budget the plan set before any of it was
written** — the image-source catalogue added 31,520 of that. That is recorded in `plans/boot-media.md` with a per-phase breakdown rather
than quietly exceeded; the figure needs re-deciding against the measurement.

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
- **`HeaderName::from_static` panics on a name that is not lower-case.** It compiles.
  At runtime it kills the connection before anything is written, so the symptom is an
  *empty response*, not an error. Put the header in the response builder, which takes any
  casing.
- **A guard against two listeners sharing a port must exempt `:0`.** Port zero asks the
  kernel for any free port, so two of them never collide — and it is what every
  integration test uses.
- **A self dev-dependency re-enables default features unless told not to.** Without
  `default-features = false`, `rescriptum = { path = "." , features = [...] }` turns
  `sqlite` and `boot` back on for every test build, so `--no-default-features` tests the
  full binary and reports coverage that does not exist.
- **`;` in an iPXE script separates commands only as a whole whitespace-delimited token**
  (`split_command` in iPXE's `core/exec.c`). So `ds=nocloud-net;s=http://…` is one
  argument and must **not** be escaped, while `foo ; bar` is two commands.
- **A PXE ROM retransmits its read request** when an answer is slow — a sleeping NAS disk
  is enough. A per-peer transfer cap is therefore a fairness bound, not a hostility
  threshold; counting malformed packets against it locks a machine out of the server it
  is retrying to reach.
- **Logging an intention is not logging an outcome.** The TFTP transfer line was written
  before the first byte went out, so a stalled transfer and a completed one looked
  identical — a machine on a real network fetched a loader, nothing happened, and the log
  said success. It is reported at the end now, `sent` or `FAILED after N of M`, with 500
  so `RESCRIPTUM_LOG=problems` keeps it.
- **Intel AMT with a static address starves the host's DHCP on a shared NIC.** The
  Proxmox installer's `dhclient` gives up after about eleven seconds; with the Management
  Engine holding the interface statically while the host asks for a lease, no offer
  arrives and the install aborts on `Network is unreachable` — while `dhclient -v eno1`
  from the installer's own shell succeeds instantly afterwards. Setting AMT to DHCP fixes
  it. Nothing here can widen that window, so it is documented rather than worked around.
- **`auto-installer-mode.toml`'s keys are snake case, and one hyphen rejects the file.**
  `AutoInstSettings` is `deny_unknown_fields`, so `partition-label` is not a warning — the
  installer refuses the whole document and stops, asking a human who is not coming. Its own
  refusal enumerates them: `mode`, `partition_label`, `http`; and inside `[http]`, `url`,
  `cert_fingerprint`, `token`. The doc comment above the writer said exactly this while the
  code wrote two hyphenated keys, and the test enumerated the hyphenated ones too.
- **In iPXE, naming an initrd turns it into a *file* rather than the initramfs.**
  `initrd <uri> <name>` gets a cpio header and lands as `/name`; `initrd <uri>` is
  appended raw and *is* the initramfs. Naming Proxmox's real initrd produced an initramfs
  holding `/initrd.img` and `/proxmox.iso` and no `/init` — the kernel unpacked 1.7 GB
  without a complaint, found nothing to run, and panicked with `VFS: Unable to mount root
  fs on unknown-block(0,0)`. **The ISO does take a name**, because the installer opens it
  by that name, so the two lines differ on purpose. `initrd=` on the command line is a
  pxelinux directive, not something the kernel resolves against a filename.
- **`Freeing initrd memory: NNNN K` means the unpacking worked.** It is the line that
  rules out every compression theory, and three guesses were spent before reading it.
- **Never acknowledge a TFTP option that is not implemented.** `windowsize` (RFC 7440) was
  echoed back in the OACK while the transfer loop sent one block and waited for its ACK. A
  client told `windowsize 4` waits for four blocks before acknowledging anything, so both
  sides waited and only the 700 ms retransmit broke the deadlock — every block costing a
  resend and 700 ms, which makes a 1.1 MB loader take **nine minutes** and the firmware
  give up first. RFC 2347 says an option left out of the OACK is treated as never
  requested, so declining costs one ACK per block and nothing else. Found on the wire, in a
  capture on the NAS, after firewalls and block sizes had both been wrongly blamed.
- **A test client that does not behave like the client it stands in for proves nothing.**
  The first version of the test for the above acknowledged every block whatever was
  negotiated, so the deadlock could not happen and it passed with the bug reintroduced. It
  honours the granted window now: 0.5 s healthy, 63 s with the bug back.
- **A `connect`ed data socket makes the kernel drop what you most need to see.** It
  filters on the peer's address *and port*, so a client that acknowledges from a different
  source port has its packets discarded before any code runs — and the transfer dies
  looking exactly like a firewall eating them, with nothing to log. RFC 1350 says a client
  keeps its TID; the ones that do not are UEFI ROMs, which is the population being served.
  The socket accepts from the same *address* now and says when the port moves.
- **A TFTP transfer leaves port 69 immediately, and that is what a firewall does not
  expect.** The server answers from a fresh port and the client acknowledges to *that*, so
  a rule allowing only 69 lets the request in, lets the answer out, and drops the
  acknowledgement — the machine then looks like it lost interest, which is as hard to read
  as this protocol gets. `RESCRIPTUM_TFTP_PORT_RANGE` pins it so it can be opened. The DSM
  package pins **30000-30063** and registers it: chosen on the machine, below the kernel's
  ephemeral range (32768–60999, so nothing there can be handed away), clear of every UDP
  port a stock DSM uses, claimed by no Synology `.sc`, and all 64 verified bindable.
- **`blksize=1468` fills a 1500-byte path exactly** — 1468 payload, 4 TFTP, 8 UDP, 20 IP —
  which is what iPXE asks for and what leaves no room at all. One VLAN tag makes the frame
  1504, and a PXE ROM meeting that usually stops without a message. `RESCRIPTUM_TFTP_BLKSIZE`
  caps it; 1400 covers a tag and most tunnels, 512 always works.
- **A TFTP transfer ends on a *short* block, and "short" includes empty.** A file whose
  length divides exactly by the block size must end with an empty data packet, or the
  client waits forever for a final block that never comes.
- **`;` separates iPXE commands only as a whole whitespace-delimited token**
  (`split_command` in iPXE's `core/exec.c`), so `ds=nocloud-net;s=…` is one argument and
  must not be escaped. And **`${version}` is iPXE's own version**, not ours.
- **`net0` is the first NIC, not the booting one** — `${netX/mac}` names the device that
  actually booted. **iPXE percent-encodes nothing on plain expansion**, so an SMBIOS
  string in a URL needs `${…:uristring}`.
- **A UEFI HTTP Boot client discards a DHCP offer that does not echo `HTTPClient`** in
  option 60. **A Windows DHCP policy cannot condition on option 93 at all** — the
  architecture reaches it only inside the option-60 string.
- **`auto-installer-mode.toml` is not a legal ISO9660 identifier.** Without a Rock Ridge
  `NM` entry the file is in the image and invisible to the installer, which looks like
  this server being broken.
- **iPXE's BIOS targets need an x86 compiler and its ARM64 ones need
  `CROSS_COMPILE=aarch64-linux-gnu-`.** Both failures read like a broken Makefile.
- **A container that died looks exactly like one still starting.** Three of the boot
  rig's first five failures hid behind that; anything that waits on a service has to
  check it is still running and print its log when it is not.
- **A service on an `internal` Docker network cannot reach apt.** Install at image-build
  time or it fails silently and the container exits 127 later.
- **dnsmasq logs to syslog unless told otherwise** (`--log-facility=-`), and a container
  has no syslog — so a marker grepping its output could never have matched.
- **A `q35` QEMU machine has no IDE controller**, so `-drive if=ide` is simply not there
  and SeaBIOS says "could not read the boot disk". Use `pc` for a BIOS guest.
- **`sed -n … "$0"` cannot find a relatively-invoked script after a `cd`.** Resolve the
  path first, or `--help` breaks for everyone who does not type an absolute path.
- **The DSM-specific traps are in `packaging/dsm/CLAUDE.md`** — the four routes to port 69
  and why `setcap` is settled, the signature gate `libsynopkg.so.1` spells out, `setcap`
  measured on the DS416j's own volume, the capability an upgrade drops, and the panel's
  runtime-computed defaults. They load with that directory; `docs/development/traps.md`
  has them at length.
- **Binding is not a health check.** A bind that *succeeds* on the TFTP port means nothing
  is listening — the degraded state, not the healthy one — and one that fails cannot tell
  this server from another daemon squatting the port, since both are `AddrInUse`. So
  `boot check` sends a real read request (`boot::tftp::probe`) and reports what a machine
  would get. The first version guessed, and a test with a squatter said so at once.
- **macOS lets an unprivileged process bind UDP port 69; Linux does not.** So a test that
  reaches the *default* TFTP address takes a different branch on each platform — `boot
  check` calls an obtainable-but-silent port a note and an unbindable one a problem, which
  is the right rule and exactly what makes the test platform-dependent. It passed locally
  and failed in CI for a reason that had nothing to do with the change. Any test that sets
  `RESCRIPTUM_BOOT_DIR` must also set `RESCRIPTUM_TFTP_ADDR=off` unless the probe *is* the
  subject; `tests/tftp.rs` covers the unbindable port on a high one.
- **A branch developed entirely offline has never met the CI.** This one accumulated 57
  commits before its first push, and the first run failed on two things no local run could
  see: a clippy five versions newer than the pinned local toolchain, and a Linux-only port
  permission. Push early enough to find out, or expect to.
- **The size figures in this file go stale.** They moved ~375 KB when armv7 changed from
  musl to glibc. Re-measure before concluding anything from them; a stale baseline once
  turned a 71% budget spend into an apparent 293% overrun.
- **A directory's mtime does not see one level down.** With a directory per identity, adding
  or editing a document *inside* a machine's directory moves nothing the listing cache
  watches, so only `RELOAD_BACKSTOP` catches it. Only the identity itself appearing or
  leaving is immediate. A test for "a removed document stops being served" that expects it
  immediately is testing the old layout and will hang on the new one.
- **An AppleDouble is worse in a directory than it was flat.** `._proxmox.toml` is now a
  *second* `.toml` in a directory that may hold only one, and `.` sorts before every letter
  — so a rule that took the first would serve a binary body to every request. `visible_name`
  is what stops it, and a test asserts the litter is not even *reported* as a conflict.
- **`git checkout <file>` to undo a deliberately-broken test also undoes the work.** Copy
  the file aside before breaking it to watch a test fail; a `git checkout` here silently
  reverted a whole feature and its test, and only a `grep` afterwards caught it.
- **The listing cache made `admin::guarded` blind over the file store.** `version()` there
  is the answers directory's mtime, which does not move when a document is written *inside*
  an existing identity's directory — so the guard compared a write against itself, found no
  difference, and kept something that broke the answer set. It forces both reads through
  `Answers::invalidate` now. Note which side matters: a stale `after` is a rollback that
  never runs and fails **open**; a stale `before` merely blames this write for a
  pre-existing problem and fails closed.
- **A `pid`-only temporary filename disambiguates processes and not threads.** Two writes
  to one document inside one process share the path, and one silently takes the other's
  content. `store::file::write_atomic` has a counter for this.
- **`log::init` ran before subcommand dispatch, and an unopenable log file is fatal.** On a
  packaged deployment `RESCRIPTUM_LOG_FILE` belongs to the service user, so any subcommand
  run by anybody else died on `cannot be opened` before its subcommand was looked at. The
  destination is now a property of *what is being run*: a server logs where the variable
  says, a subcommand logs to stderr. `Config::validate` deliberately stayed fatal for
  both — `tests/tftp.rs` pins that.
- **A group's `.ipxe` arms machines that can never be disarmed.** `installed::disarm` moves
  a *machine's* own document and never a group's, so a machine armed only by its group
  installs, reports success, is not disarmed, and installs again on its next network boot —
  while the webhook logs `nothing was claiming it`, which reads like it worked. The
  project's own `examples/groups/edge-router/boot.ipxe` did this and `check` called it
  green; `check` reports it now.

## Layout on disk, and the core algorithm

**One directory per identity.** A machine is a directory in `RESCRIPTUM_ANSWERS_DIR` named
after it, holding one document per format; `groups/` and `default/` are the same shape under
names the layout reserves (`valid_machine_id` refuses both as machine ids, in *both* stores,
so `export` from SQLite can always be represented).

Inside a directory, **the extension is the format and the stem is nothing at all** —
`proxmox.toml` and `answer.toml` are one document. `format::canonical_stem` picks a readable
name for a document nobody has named; an existing one is overwritten *where it stands*, so
an operator's name survives a write. Two documents of one format in one directory is a
**reported problem**, not a resolved one: there is no tiebreak anyone could predict. Sorted
order decides which answers so the choice does not depend on readdir, and the loser is named.

A servable document left flat at the top — the layout before this one — is **reported and
not served**, its destination spelled out, and `rescriptum migrate [--apply]` moves them.
`store::file::pending_moves` is that knowledge exposed once, so the command and the reader
cannot disagree. Half-reading an old layout would mean a machine whose answer moved silently
between two files.

Selection per request:

1. Normalize the request body: lowercase, strip every non-alphanumeric character.
2. For each identity directory (excluding `groups/` and `default/`), normalize its name the
   same way and test whether it appears as a substring of the normalized body.
3. First match wins → return that identity's document for the format asked for.
4. No match → `default/` if it holds that format, else `404`.

The normalization is what makes this robust: it's indifferent to MAC separator style
(`98-fa-9b…` / `98:fa:9b…` / `98fa9b…`) and to the JSON structure, which changes between Proxmox
versions.

The listing is **cached and invalidated by the directory's mtime**, which changes whenever a
file is added or removed. The spec asks for a re-read on every request; done literally that is a
`readdir` plus a sort plus a normalization pass per request, and with one answer file per
machine — the datacenter case — throughput collapses. Measured, 3000 requests at 100 concurrent:

| Machines in the directory | Literal re-read | mtime-cached |
|---|---|---|
| 10 | 11,954 req/s | 12,922 req/s |
| 200 | 3,198 req/s | 12,890 req/s |
| 2,000 | 311 req/s | 12,520 req/s |
| 10,000 | — | 6,924 req/s |

One `stat` replaces the whole walk, and a new machine is still picked up with no restart —
which is the guarantee the spec actually wanted. `RELOAD_BACKSTOP` (1 s) forces a re-read even
when mtime looks unchanged, covering filesystems with coarse mtime granularity. Normalized
identities are computed once per directory read, not once per request.

**The mtime now sees less, and the read costs more.** A directory's mtime moves when an entry
is added or removed *in it*, so an identity appearing or leaving is immediate while a document
added or edited **inside** one is only caught by the backstop — the same rule that already
covered a file edited in place, now the normal case. And a full reload is a `readdir` per
identity on top of the file it already opened: measured at 2,000 machines on an M1 Pro,
**28.6 ms flat → 63.5 ms with a directory each (2.2×)**, unchanged by removing the
allocations, because it is syscalls. Amortised over a second of requests it did not move
end-to-end throughput measurably; the req/s table above was measured against the flat layout
and has *not* been re-measured with a comparable tool. It is still the reason to reach for a
group before a directory per machine.

The remaining cost at 10,000 machines is the linear scan of precomputed needles — pure CPU, no
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
- Only read the answers directory and one level below it. Never build a filesystem path from
  request data — that is the path-traversal guard.

Logging goes to stdout/stderr, one line per request (timestamp, source IP, body size, chosen
file or failure). When a PXE install won't start, this is the only diagnostic available.

## Configuration

Environment variables — plus an optional file to read them from, in either of two shapes.
Both files set the same variables under the same rules; **the environment wins over both,
and the TOML file wins over the env file**:

| Variable | Default | Role |
|---|---|---|
| `RESCRIPTUM_CONFIG` | unset | A **TOML** file of the same settings, under readable names. See below |
| `RESCRIPTUM_ENV_FILE` | unset | A `KEY=value` file of the same variables. See below |
| `RESCRIPTUM_STORE` | `files` | `files` or `sqlite` |
| `RESCRIPTUM_ANSWERS_DIR` | `/srv/answers` | Directory of answer documents |
| `RESCRIPTUM_DB_PATH` | `/srv/answers.db` | SQLite database, when `RESCRIPTUM_STORE=sqlite` |
| `RESCRIPTUM_LISTEN_ADDR` | `0.0.0.0:8000` | Listen address (`:0` picks a free port) |
| `RESCRIPTUM_WORKERS` | CPU count | tokio runtime threads (not a concurrency limit) |
| `RESCRIPTUM_MAX_CONNECTIONS` | `2048` | In-flight connections before shedding with 503 |
| `RESCRIPTUM_TIMEOUT_SECS` | `10` | Header-read timeout **and** whole-connection deadline |
| `RESCRIPTUM_LOG` | `all` | `all` \| `problems` (drops the requests that worked) \| `off` |
| `RESCRIPTUM_LOG_FILE` | unset | A file to append to, or `stdout`/`stderr`. Unopenable is fatal |
| `RESCRIPTUM_MEDIA_DIR` | unset | Installer images. **Unset is the whole off switch for boot media** |
| `RESCRIPTUM_MEDIA_ADDR` | `0.0.0.0:8001` | The media listener, when there is a media directory |
| `RESCRIPTUM_MEDIA_TIMEOUT_SECS` | `600` | Whole-transfer deadline — deliberately not the answer listener's 10 |
| `RESCRIPTUM_MEDIA_MAX_CONNECTIONS` | `16` | Concurrent transfers; low on purpose |
| `RESCRIPTUM_PUBLIC_HOST` | the routing table's answer, else a sole interface | The host generated URLs name. **A host, never a URL**. Warns and names the alternatives when the host has several |
| `RESCRIPTUM_BOOT_ALLOW` | unset | Client CIDRs allowed to fetch boot media |
| `RESCRIPTUM_BOOT_UNCLAIMED` | `menu` | Or `local`. **Inverts what an answer file means**: with `local`, present is *install this one* and absent is the safe state |
| `RESCRIPTUM_INSTALLED_TOKEN` | unset | Proxmox's webhook token. Set it and `POST /installed` drops a machine's `.ipxe` claim when it reports success. **Unset, the endpoint does not exist** |
| `RESCRIPTUM_BOOT_DIR` | unset | Loaders and menus. **Unset means no TFTP at all** |
| `RESCRIPTUM_TFTP_ADDR` | `0.0.0.0:69` when `RESCRIPTUM_BOOT_DIR` is set | Or `off`, a deployment workaround, never a packaged default. A failed bind here warns rather than killing the server |

A zero or unparseable numeric value falls back to the default rather than starting a server
that accepts and never answers.

**What is fatal and what is a warning is deliberate.** Fatal: the listener cannot bind, the
store cannot be opened (SQLite catches an unwritable directory, a corrupt file and a
too-new schema at open, not at the first request), a named env or log file cannot be read,
and any unsafe admin combination — **except the TFTP one**, which is the single exception
and is measured rather than argued: port 69 is the only privileged port in the design, so
it is the only bind that can fail for something nobody configured, and dying there takes
answers and media with it. A warning: a failed TFTP bind, the answers directory being
absent, not a directory, or unlistable, and any problem in the answer set — all fixable
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

**`RESCRIPTUM_CONFIG` (`src/tomlconfig.rs`)** is the same job in the shape a person reads:
the prefix goes away and tables do the grouping (`store.kind`, `admin.token`,
`server.workers`). It exists because on DSM there is no environment — there is a file — and
`RESCRIPTUM_ANSWERS_DIR=…` on every line is a poor thing to hand somebody editing in File
Station. Every rule above carries over unchanged, and three things are specific to it:

- **It costs a mapping, not a dependency.** `toml_edit` already parses every answer
  document. Measured on armv7: **+14,544 bytes** (2,799,168 → 2,813,712), 0.5%.
- **`""` is unset**, the same rule an exported-but-empty variable has — which is what lets
  `config unset` empty a line instead of deleting the paragraph documenting it. A list or a
  table where a value belongs is a startup *error*, unlike a misspelled key, which warns:
  it was aimed at a real setting, so serving the default would be the silent failure.
- **A configuration file must not live in the answers directory.** Every servable `.toml`
  at the top of that directory is an answer document, and this format shares the extension
  — `check` reports one dropped there as a misplaced answer and `migrate` offers to move
  it. A test pins that rather than leaving it to be discovered.

## Commands

Local development (once the crate exists):

```bash
cargo run -- check               # validate an answers directory
cargo run -- migrate             # show what a flat answers directory would become
cargo run -- migrate --apply     # move those documents into a directory each
cargo run -- config              # show the configuration and where each value comes from
cargo run -- render <mac>        # print one machine's composed answer
cargo run -- media list          # the installer images held
cargo run -- media add FILE      # register one: verify, probe, record its digest
cargo run -- media check         # re-verify every recorded digest
cargo run -- media ipxe ID       # the .ipxe answer that boots one image
```

Documentation (see *Documentation* below):

```bash
npm install                      # once — the docs toolchain only
npm run docs                     # review server at http://localhost:3009
npm run docs:build               # the public artifact into ./_site
npm run docs:lint                # no broken internal links (CI gate)
```

Cross-compile for the NAS (ARMv7 hard-float — **glibc, not musl**, see below):

```bash
cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.17
```

**armv7 is the one target that is not static musl, and it is not a preference.** Synology's
ARMv7 kernels are 3.10 and answer the *time64* syscalls with `EINVAL` rather than `ENOSYS`;
musl 1.2 falls back to the 32-bit syscalls only on `ENOSYS`, so every `clock_gettime`,
`clock_nanosleep` and timed futex fails. Measured on a DS416j: the musl build installs,
answers `--version`, and panics at `time.rs:131` with `Os { code: 22, kind: InvalidInput }`
the moment it wants a timestamp. glibc on 32-bit uses time32, DSM ships 2.20 on
`armada38x`, and glibc is backward compatible — so the floor is **2.17** and the same binary
still runs on newer ARMv7 Linux. x86_64 and aarch64 are 64-bit, have no time32/time64 split,
and stay static musl. What CI asserts for armv7 is therefore *not* "static" but "needs no
glibc newer than 2.17".

`cargo-zigbuild` uses Zig as the linker, avoiding a full cross toolchain. The toolchain is
**installed and verified**: Rust 1.93, targets `aarch64-apple-darwin` +
`armv7-unknown-linux-gnueabihf`, `cargo-zigbuild` 0.23.0, Zig 0.16.0. A scratch crate built
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

## The DSM package

`packaging/dsm/` wraps an already-built binary as a DSM 7 `.spk`. It is a **release
format**, exactly like the `.tar.gz` archives — no DSM-specific build, no feature flag,
nothing in `src/` knows it exists, and if it ever seems to need a `#[cfg]`, the design has
gone wrong. It carries a real DSM desktop application for the settings panel, and it ships
the branded loaders, so the share's `boot` folder arrives filled.

**Changing anything under `packaging/dsm/` means running the machine**, not just the local
harness. `packaging/dsm/CLAUDE.md` holds the whole contract — the four places DSM pressed
back, the privilege routes measured on a DS416j and the DS416j run itself, the desktop
application, the lifecycle rules and the DSM traps — and it loads whenever you touch that
directory. The procedure is in `packaging/dsm/vm/README.md` (*Changing the package? This
is the procedure*), which `AGENTS.md` also points at.

## Testing expectations

619 tests, plus the package's own harnesses (see *The DSM package*, and note that
`cargo test` does not run those). `docs/development/testing.md` has the per-suite table;
the rules that decide where a test goes:

- **The whole boot chain belongs in `packaging/boot-rig/`**, which is not Rust and which
  `cargo test` does not run. `run.sh` boots a claimed and an unclaimed machine in QEMU
  and asserts four markers; CI runs the same thing plus one deliberate break. **A QEMU
  guest bridged into a container has a MAC of its own, and Docker Desktop's virtual
  switch does not forward frames from a MAC it did not assign** — measured, which is why
  the primary rig is one container on a private bridge rather than four on a Docker
  network.
- **TFTP belongs in `tests/tftp.rs`**, speaking the protocol over real UDP. A transfer is
  a conversation, and every bug worth catching lives in the turn-taking: the first run
  found two, both of the "works by hand, never after a reboot" kind.
- **Boot media belongs in `tests/media.rs`**, against the real binary with both listeners
  up. Every abuse case there ends by proving the server still answers, and one case proves
  the property the separate socket exists for: **answers keep succeeding while four image
  transfers are in flight**. There is deliberately **no binary ISO fixture in this
  repository** — `boot::iso::build` writes images in memory, behind the `test-support`
  feature so it never reaches a release binary.

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
two audiences: `docs/guide/` is **using** rescriptum, `docs/development/` is **working on**
it. Nothing in `src/` knows the site exists.

This file and `docs/development/` overlap deliberately — this one is condensed for agents,
that one is written for a human reading in order. **A change to a constraint belongs in
both**, and a user-visible change should land with its documentation in the same PR.

`docs/CLAUDE.md` holds the rest — the site configuration, the FR mirroring rules, the
writing conventions and the anchor trap — and loads whenever you touch `docs/`.

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
GitHub Release with cross-compiled binaries attached, built by a CI matrix, plus a `.spk`
per Linux ABI from the `package-dsm` job and **`rescriptum-boot-assets-<version>.tar.gz`
from the `loaders` job** — the branded iPXE loaders, their own download because they are
GPLv2 and `packaging/ipxe/` is the written offer. Without it a release ships a TFTP server
with nothing to hand out. A `spk_build` dispatch input ships a
packaging-only fix as `0.1.0-2` without a new tag. Submission to SynoCommunity may follow
later; a package source that Package Center could poll deliberately will not — there are no
update notifications, and the documentation says so.

CI gates on every push: **gates** (fmt, clippy, tests, the no-SQLite build), **docs** (public
build plus `notabene lint`), **audit** (`cargo audit --deny warnings`, which fails on an
unmaintained or yanked crate as well as a vulnerability — an unfixable one gets
`--ignore RUSTSEC-…` with a reason, not the flag removed), and **cross** (ARMv7-musl, then
asserting it needs no glibc newer than the floor DSM has, then assembling both `.spk`s and checking them
structurally). Every action is an official `actions/*`; Zig and
`cargo-audit` are installed directly, because this toolchain vets and links a binary people
run as root.

**`develop` publishes nothing.** It runs the CI gates (build, tests, clippy, fmt) and stops
there — no prereleases, no artifacts. Binaries are produced only by a `vX.Y.Z` tag on `main`.

Release target matrix (settled):

| Target | For |
|---|---|
| `armv7-unknown-linux-gnueabihf` (floor 2.17) | the DS416j, the reason this project exists |
| `aarch64-unknown-linux-musl` | modern ARM NAS / Raspberry Pi |
| `x86_64-unknown-linux-musl` | most other Linux hosts |
| `aarch64-apple-darwin` | local development |
| `x86_64-apple-darwin` | local development |

## Project conventions

This **is** a public open-source GitHub project — <https://github.com/z29k/rescriptum>, MIT,
with the documentation site at <https://z29k.github.io/rescriptum/> — serving both personal
and professional use, so treat robustness, release management across branches, and
documentation quality as requirements rather than polish. Being public is not a milestone
still ahead: it is the condition every change now lands under. A force-push is immediate and
irreversible, and so is a published release.

The spec's "README covering purpose, cross-compilation, DSM deployment, ISO preparation
and troubleshooting" is now `docs/` instead — the README had grown to 28 KB and was three
documents wearing one coat. It is a landing page linking into the site.

Out of scope but documented rather than implemented: TLS (plain HTTP is fine on a
trusted LAN — document the workaround for installer versions demanding a cert fingerprint) and
URL discovery via DNS TXT / DHCP option 250.

Prior art worth consulting: <https://github.com/SlothCroissant/proxmox-auto-installer-server>
Proxmox reference: <https://pve.proxmox.com/wiki/Automated_Installation>
