//! Choosing which answer to serve, and composing it from its layers.
//!
//! Two ways to claim a machine, and they are ordered by how narrowly they target it:
//!
//! * **By identity** — a document named after the machine, or a group listing it in
//!   `members`. Naming a machine is as specific as it gets, so it always wins.
//! * **By selector** — a `match` block tested against the facts of the request (see
//!   `facts.rs`). Among selectors, the one satisfying more criteria wins.
//!
//! Ties break on sorted name, so the answer never depends on filesystem or row order.
//! matchbox, the closest prior art, documents that its own resolution between competing
//! groups "will not be deterministic"; this one is.

use crate::facts::Facts;
use crate::format::{self, Control, Doc, Kind};
use crate::store::{Snapshot, Store};
use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Re-read at least this often even when the store's version looks unchanged. A file's
/// *contents* can change without moving any directory mtime, and another process can
/// write to the database — this bounds how long either can go unnoticed.
const RELOAD_BACKSTOP: Duration = Duration::from_secs(1);

/// Naming a machine outranks any selector, however many criteria the selector carries.
const IDENTITY_SCORE: u32 = 1_000;

/// What was served, and what it was built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The document to send to the installer.
    pub body: String,
    /// What to label it as.
    pub content_type: &'static str,
    /// Machine identifier, when one matched.
    pub machine: Option<String>,
    /// Group name, when one applied.
    pub group: Option<String>,
    /// Whether this came from the `default` document.
    pub used_default: bool,
    /// The format family everything was composed in.
    pub format: Kind,
    /// Its extension — `ks` and `preseed` share a family but not a validator, so this
    /// is what `check` needs in order to call the right tool.
    pub format_name: String,
}

impl Resolution {
    /// A compact description for the log line.
    pub fn how(&self) -> String {
        let mut parts = vec![format!("format={}", self.format.label())];
        if let Some(m) = &self.machine {
            parts.push(format!("machine={m}"));
        }
        if let Some(g) = &self.group {
            parts.push(format!("group={g}"));
        }
        if self.used_default {
            parts.push("default".to_string());
        }
        parts.join(" ")
    }
}

/// Reduce input to lowercase ASCII alphanumerics, dropping everything else.
///
/// Takes bytes rather than `&str` on purpose: a request body is arbitrary bytes and
/// need not be valid UTF-8. Filtering to ASCII alphanumerics sidesteps the question
/// entirely — no validation, no lossy conversion, no failure mode.
pub fn normalize(input: &[u8]) -> String {
    input
        .iter()
        .filter(|b| b.is_ascii_alphanumeric())
        .map(|b| b.to_ascii_lowercase() as char)
        .collect()
}

/// How strongly a rule claims this request, or `None` if it does not.
fn score(control: &Control, identity: &[String], facts: &Facts) -> Option<u32> {
    // Naming the machine is the most specific thing anyone can do.
    if identity
        .iter()
        .any(|needle| !needle.is_empty() && facts.haystack().contains(needle))
    {
        return Some(IDENTITY_SCORE);
    }
    if control.matchers.is_empty() {
        return None;
    }
    // Every criterion must hold; the more there are, the more deliberate the rule.
    control
        .matchers
        .iter()
        .all(|(key, pattern)| facts.matches(key, pattern))
        .then_some(control.matchers.len() as u32)
}

struct Candidate {
    id: String,
    /// The document's extension. What distinguishes this machine's Proxmox answer from
    /// its Debian one, and what an endpoint filters on.
    format: String,
    /// The identifier, already normalized — computed once per load rather than once per
    /// request, which matters when there are thousands of machines.
    identity: Vec<String>,
    kind: Kind,
    /// Parsed once per load. A document that will not parse is kept as the error, so a
    /// machine whose own file is broken gets a loud 500 rather than silently falling
    /// through to a group or the default and installing the wrong thing.
    doc: Result<Doc, String>,
    control: Control,
}

pub struct Group {
    pub name: String,
    pub format: String,
    /// Whether `rendered` can be served as-is, or the document has to be filled first.
    has_placeholders: bool,
    pub origin: String,
    kind: Kind,
    identity: Vec<String>,
    control: Control,
    /// The group's own content merged onto its `extends` chain.
    merged: Doc,
    /// The above, control keys stripped and rendered. Serving a group with no machine
    /// overrides then costs no parsing at all — the common datacenter case.
    rendered: String,
}

struct Listing {
    candidates: Vec<Candidate>,
    groups: Vec<Group>,
    /// One per format: a TOML default must not answer a client that asked for
    /// kickstart.
    fallbacks: Vec<(String, Kind, Result<Doc, String>)>,
    /// Configuration mistakes found while loading. Reported once per load rather than
    /// once per request.
    problems: Vec<String>,
}

/// Expand any `{{ key }}` the composed answer carries.
///
/// The variables are the request's own facts, plus `machine` and `group` — which the
/// facts cannot carry, because they are only known once matching has happened. That is
/// what lets one group cover a whole rack: `fqdn = "{{ machine }}.example.com"` needs no
/// file per machine.
fn fill(
    doc: &mut Doc,
    facts: &Facts,
    machine: Option<&str>,
    group: Option<&str>,
) -> io::Result<()> {
    if !doc.has_placeholders() {
        return Ok(());
    }
    doc.substitute(&|key: &str| match key {
        "machine" => machine.map(str::to_string),
        "group" => group.map(str::to_string),
        other => facts.values(other).first().cloned(),
    })
    .map_err(io::Error::other)
}

/// The formats this request will accept, from the endpoint it came in on.
///
/// `None` means it said nothing, so anything may answer — which is what keeps a URL
/// like `/answer` working for a deployment that only ever serves one format.
fn wanted(facts: &Facts) -> Option<&'static [&'static str]> {
    facts
        .values("segment")
        .iter()
        .find_map(|segment| format::endpoint_formats(segment))
}

fn acceptable(wanted: Option<&'static [&'static str]>, format: &str) -> bool {
    match wanted {
        None => true,
        Some(formats) => formats.iter().any(|f| f.eq_ignore_ascii_case(format)),
    }
}

impl Listing {
    /// Inheritance looks for a group by name, within the same format — layering a
    /// preseed onto a TOML base is meaningless, and `Doc::merge` would refuse it anyway.
    fn group_named(&self, format: &str, name: &str) -> Option<&Group> {
        self.groups
            .iter()
            .find(|g| g.name == name && g.format.eq_ignore_ascii_case(format))
    }

    /// The best claim on this request, if any. Ties break on sorted name.
    fn best_group(&self, facts: &Facts) -> Option<&Group> {
        let wanted = wanted(facts);
        self.groups
            .iter()
            .filter(|g| acceptable(wanted, &g.format))
            .filter_map(|g| score(&g.control, &g.identity, facts).map(|s| (s, g)))
            .max_by(|(a, ga), (b, gb)| a.cmp(b).then_with(|| gb.name.cmp(&ga.name)))
            .map(|(_, g)| g)
    }

    fn best_machine(&self, facts: &Facts) -> Option<&Candidate> {
        let wanted = wanted(facts);
        self.candidates
            .iter()
            .filter(|c| acceptable(wanted, &c.format))
            .filter_map(|c| score(&c.control, &c.identity, facts).map(|s| (s, c)))
            .max_by(|(a, ca), (b, cb)| a.cmp(b).then_with(|| cb.id.cmp(&ca.id)))
            .map(|(_, c)| c)
    }

    fn fallback_for(&self, facts: &Facts) -> Option<&(String, Kind, Result<Doc, String>)> {
        let wanted = wanted(facts);
        self.fallbacks
            .iter()
            .find(|(format, _, _)| acceptable(wanted, format))
    }
}

struct Cached {
    version: Option<String>,
    loaded_at: Instant,
    listing: Arc<Listing>,
}

/// The answer set, with its parsed contents cached between requests.
pub struct Answers {
    store: Arc<dyn Store>,
    cache: Mutex<Option<Cached>>,
}

impl Answers {
    pub fn new(store: Arc<dyn Store>) -> Answers {
        Answers {
            store,
            cache: Mutex::new(None),
        }
    }

    /// Convenience for the file layout.
    pub fn from_dir(dir: impl Into<std::path::PathBuf>) -> Answers {
        Answers::new(Arc::new(crate::store::FileStore::new(dir)))
    }

    pub fn describe(&self) -> String {
        self.store.describe()
    }

    pub fn problems(&self) -> io::Result<Vec<String>> {
        Ok(self.listing()?.problems.clone())
    }

    /// A group's format, so a caller can address it the way its installer would.
    pub fn group_format(&self, name: &str) -> io::Result<Option<String>> {
        Ok(self
            .listing()?
            .groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.format.clone()))
    }

    /// Groups currently loaded, as (name, origin), sorted by name.
    pub fn group_names(&self) -> io::Result<Vec<(String, String)>> {
        Ok(self
            .listing()?
            .groups
            .iter()
            .map(|g| (g.name.clone(), g.origin.clone()))
            .collect())
    }

    /// Machines as (identifier, format), sorted — a machine that exists for two
    /// operating systems appears once per format.
    pub fn machine_documents(&self) -> io::Result<Vec<(String, String)>> {
        Ok(self
            .listing()?
            .candidates
            .iter()
            .map(|c| (c.id.clone(), c.format.clone()))
            .collect())
    }

    /// Identifiers of the machines currently loaded, sorted.
    pub fn machine_ids(&self) -> io::Result<Vec<String>> {
        Ok(self
            .listing()?
            .candidates
            .iter()
            .map(|c| c.id.clone())
            .collect())
    }

    /// Drop the cached listing, so the next read goes back to the store.
    ///
    /// **This exists for `admin::guarded`, and the rollback depends on it.** The guard
    /// compares `problems()` before and after a write, but over the file store `version()`
    /// is the answers directory's mtime — which does not move when a document is written
    /// *inside* an existing identity's directory, and which a coarse-granularity
    /// filesystem may not move even for a new one within the same second. Either way the
    /// listing is served from cache for up to `RELOAD_BACKSTOP`, so a write that broke the
    /// answer set would compare equal to itself, pass the guard, and be kept.
    ///
    /// Which side is forced matters, and only one of them has to be: a stale `after` is a
    /// rollback that never runs and fails **open**, while a stale `before` blames this
    /// write for a pre-existing problem and fails **closed**. The guard forces both, since
    /// the safe direction is cheap to buy twice.
    pub fn invalidate(&self) {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
    /// A group's selector criteria, if it has any.
    pub fn group_matchers(&self, name: &str) -> io::Result<Vec<(String, String)>> {
        Ok(self
            .listing()?
            .groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| {
                g.control
                    .matchers
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Members of a group, normalized.
    pub fn group_members(&self, name: &str) -> io::Result<Vec<String>> {
        Ok(self
            .listing()?
            .groups
            .iter()
            .find(|g| g.name == name)
            .map(|g| g.identity.clone())
            .unwrap_or_default())
    }

    /// Work out what to serve for this request.
    ///
    /// Returns `None` when nothing applies, which the caller turns into a 404.
    pub fn resolve(&self, facts: &Facts) -> io::Result<Option<Resolution>> {
        let listing = self.listing()?;

        let machine = listing.best_machine(facts);
        // Inheritance stays inside one format: layering a preseed onto a TOML base is
        // meaningless, and the merge would refuse it anyway.
        let machine_format = machine.map(|c| c.format.as_str()).unwrap_or("");
        let machine_doc = match machine {
            Some(c) => Some(c.doc.as_ref().map_err(|e| io::Error::other(e.clone()))?),
            None => None,
        };

        // An explicit `extends` in the machine document beats group membership: it is
        // the escape hatch for a machine that needs a group it is not listed in.
        let group = match machine.and_then(|c| c.control.extends.clone()) {
            Some(name) => match listing.group_named(machine_format, &name) {
                Some(g) => Some(g),
                None => {
                    return Err(io::Error::other(format!(
                        "machine {:?} extends unknown group {name:?}",
                        machine.map(|c| c.id.as_str()).unwrap_or_default()
                    )));
                }
            },
            None => listing.best_group(facts),
        };

        let resolution = match (machine, machine_doc, group) {
            // Group only: nothing to merge. When it carries no placeholder the string
            // prepared at load is served as-is, which is the common case and costs no
            // parsing at all.
            (None, _, Some(g)) => {
                let body = if g.has_placeholders {
                    let mut doc = g.merged.clone();
                    fill(&mut doc, facts, None, Some(&g.name))?;
                    doc.strip_control();
                    doc.render()
                } else {
                    g.rendered.clone()
                };
                Resolution {
                    body,
                    content_type: g.kind.content_type(),
                    machine: None,
                    group: Some(g.name.clone()),
                    used_default: false,
                    format: g.kind,
                    format_name: g.format.clone(),
                }
            }

            (Some(c), Some(doc), None) => {
                let mut doc = doc.clone();
                fill(&mut doc, facts, Some(&c.id), None)?;
                doc.strip_control();
                Resolution {
                    body: doc.render(),
                    content_type: c.kind.content_type(),
                    machine: Some(c.id.clone()),
                    group: None,
                    used_default: false,
                    format: c.kind,
                    format_name: c.format.clone(),
                }
            }

            // Both: the group underneath, the machine on top.
            (Some(c), Some(doc), Some(g)) => {
                let mut merged = g.merged.clone();
                merged.merge(doc).map_err(|e| {
                    io::Error::other(format!("machine {:?} over group {:?}: {e}", c.id, g.name))
                })?;
                fill(&mut merged, facts, Some(&c.id), Some(&g.name))?;
                merged.strip_control();
                Resolution {
                    body: merged.render(),
                    content_type: c.kind.content_type(),
                    machine: Some(c.id.clone()),
                    group: Some(g.name.clone()),
                    used_default: false,
                    format: c.kind,
                    format_name: c.format.clone(),
                }
            }

            // Nothing matched: fall back, if there is one.
            _ => match listing.fallback_for(facts) {
                Some((_, _, Err(e))) => return Err(io::Error::other(e.clone())),
                Some((format, kind, Ok(doc))) => {
                    let mut doc = doc.clone();
                    // The default document may extend a group too.
                    if let Some(name) = doc.control().extends {
                        let Some(g) = listing.group_named(format, &name) else {
                            return Err(io::Error::other(format!(
                                "default extends unknown group {name:?}"
                            )));
                        };
                        let mut merged = g.merged.clone();
                        merged
                            .merge(&doc)
                            .map_err(|e| io::Error::other(format!("default over {name:?}: {e}")))?;
                        doc = merged;
                    }
                    fill(&mut doc, facts, None, None)?;
                    doc.strip_control();
                    Resolution {
                        body: doc.render(),
                        content_type: kind.content_type(),
                        machine: None,
                        group: None,
                        used_default: true,
                        format: *kind,
                        format_name: format.clone(),
                    }
                }
                None => return Ok(None),
            },
        };
        Ok(Some(resolution))
    }

    fn listing(&self) -> io::Result<Arc<Listing>> {
        let version = self.store.version();

        // A poisoned lock means some other request panicked mid-refresh. The cached
        // data is still structurally fine, so carry on rather than failing the install.
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());

        // `version.is_some()` cannot fire with either store as they stand: the file store
        // reports None only when the directory is missing, and a missing directory also
        // snapshots empty, while SQLite always reports its atomic. It is kept because the
        // rule it states is the correct one — an unreadable version is not evidence that
        // nothing changed — and a third store could easily have both. Nothing tests it,
        // and nothing can until such a store exists.
        if let Some(cached) = guard.as_ref()
            && cached.version == version
            && version.is_some()
            && cached.loaded_at.elapsed() < RELOAD_BACKSTOP
        {
            return Ok(Arc::clone(&cached.listing));
        }

        let listing = Arc::new(build(self.store.snapshot()?));
        // Logged here rather than by the caller, so a problem that appears at runtime is
        // reported the same way as one present at startup, and only once either way.
        for problem in &listing.problems {
            crate::log::server(&format!("warning: {problem}"));
        }
        *guard = Some(Cached {
            version,
            loaded_at: Instant::now(),
            listing: Arc::clone(&listing),
        });
        Ok(listing)
    }
}

fn kind_of(format: &str, what: &str, problems: &mut Vec<String>) -> Option<Kind> {
    match Kind::for_extension(format) {
        Some(kind) => Some(kind),
        None => {
            problems.push(format!("{what}: unknown format {format:?}"));
            None
        }
    }
}

/// Turn raw documents into the form requests are answered from.
fn build(snapshot: Snapshot) -> Listing {
    let mut problems = snapshot.problems;

    // Parse every group, then resolve each one's `extends` chain.
    let parsed: Vec<(String, String, String, Kind, Doc)> = snapshot
        .groups
        .into_iter()
        .filter_map(|g| {
            let kind = kind_of(&g.format, &g.origin, &mut problems)?;
            match Doc::parse(kind, &g.body, &g.origin) {
                Ok(doc) => Some((g.name, g.format, g.origin, kind, doc)),
                // A broken group must not take the whole directory down; report it and
                // carry on, so the other racks still install.
                Err(e) => {
                    problems.push(e);
                    None
                }
            }
        })
        .collect();

    let mut groups: Vec<Group> = Vec::new();
    for (name, format, origin, kind, doc) in &parsed {
        let mut chain: Vec<&Doc> = Vec::new();
        let mut seen = HashSet::new();
        let mut current = name.clone();
        let mut broken = false;

        loop {
            if !seen.insert(current.clone()) {
                problems.push(format!(
                    "group {name:?}: `extends` cycle through {current:?}"
                ));
                broken = true;
                break;
            }
            // A group inherits from one of its own format; anything else could not
            // be merged into it.
            let Some((_, _, _, parent_kind, parent_doc)) = parsed
                .iter()
                .find(|(n, f, _, _, _)| *n == current && f.eq_ignore_ascii_case(format))
            else {
                problems.push(format!("group {name:?}: extends unknown group {current:?}"));
                broken = true;
                break;
            };
            if parent_kind != kind {
                problems.push(format!(
                    "group {name:?} is {} but extends {current:?} which is {} — every layer of \
                     one answer must be the same format",
                    kind.label(),
                    parent_kind.label()
                ));
                broken = true;
                break;
            }
            chain.push(parent_doc);
            match parent_doc.control().extends {
                Some(parent) => current = parent,
                None => break,
            }
        }
        if broken {
            continue;
        }

        // The chain runs child → … → root; merge from the root down so the child wins.
        let mut merged = chain[chain.len() - 1].clone();
        for doc in chain.iter().rev().skip(1) {
            if let Err(e) = merged.merge(doc) {
                problems.push(format!("group {name:?}: {e}"));
                break;
            }
        }

        let control = doc.control();
        let identity: Vec<String> = control
            .members
            .iter()
            .map(|m| normalize(m.as_bytes()))
            .filter(|n| !n.is_empty())
            .collect();

        let mut rendered_doc = merged.clone();
        rendered_doc.strip_control();

        groups.push(Group {
            name: name.clone(),
            format: format.clone(),
            has_placeholders: merged.has_placeholders(),
            origin: origin.clone(),
            kind: *kind,
            identity,
            control,
            merged,
            rendered: rendered_doc.render(),
        });
    }
    groups.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.format.cmp(&b.format)));

    let mut candidates: Vec<Candidate> = snapshot
        .machines
        .into_iter()
        .filter_map(|m| {
            let kind = kind_of(&m.format, &m.origin, &mut problems)?;
            let format = m.format.clone();
            // The origin, not the id: with a directory per identity the filename is the
            // operator's to choose, so "98fa9b50d810" would not say which document in it
            // failed to parse.
            let doc = Doc::parse(kind, &m.body, &m.origin);
            if let Err(e) = &doc {
                problems.push(e.clone());
            }
            let control = doc.as_ref().map(|d| d.control()).unwrap_or_default();
            // An identifier of only punctuation normalizes to "", and "".contains() is
            // true for every body — it would match every machine. Drop it here.
            let needle = normalize(m.id.as_bytes());
            let identity = if needle.is_empty() {
                Vec::new()
            } else {
                vec![needle]
            };
            Some(Candidate {
                id: m.id,
                format,
                identity,
                kind,
                doc,
                control,
            })
        })
        .collect();

    // Store order must not decide behaviour.
    candidates.sort_by(|a, b| a.id.cmp(&b.id).then_with(|| a.format.cmp(&b.format)));

    // A document pointing at a group that does not exist only fails when that machine
    // asks — long after whoever wrote it has moved on. Surface it at load instead, so
    // `check` reports it and the admin API can refuse the write that caused it.
    let known: HashSet<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    for candidate in &candidates {
        if let Some(parent) = &candidate.control.extends
            && !known.contains(parent.as_str())
        {
            problems.push(format!(
                "machine {:?}: extends unknown group {parent:?}",
                candidate.id
            ));
        }
    }

    let fallbacks: Vec<(String, Kind, Result<Doc, String>)> = snapshot
        .fallbacks
        .into_iter()
        .filter_map(|d| {
            let what = d.origin.clone();
            let kind = kind_of(&d.format, &what, &mut problems)?;
            let doc = Doc::parse(kind, &d.body, &what);
            match &doc {
                Err(e) => problems.push(e.clone()),
                Ok(parsed) => {
                    if let Some(parent) = parsed.control().extends
                        && !known.contains(parent.as_str())
                    {
                        problems.push(format!("{what}: extends unknown group {parent:?}"));
                    }
                }
            }
            Some((d.format, kind, doc))
        })
        .collect();

    Listing {
        candidates,
        groups,
        fallbacks,
        problems,
    }
}

/// Re-exported so callers do not have to reach into `format` for the common case.
pub use format::Kind as Format;
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique temp directory, removed on drop. Avoids a dev-dependency on `tempfile`.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("rescriptum-test-{}-{}", std::process::id(), n));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Write one document, named the way a test thinks of it — `<id>.<ext>` — into
        /// the directory the layout actually stores it in.
        ///
        /// The translation lives here rather than in every test because what these
        /// fixtures are about is *which machine, in which format*; the filename inside
        /// the directory is not a thing any of them mean to assert.
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            self.document(&self.0, name)
                .tap(|p| fs::write(p, contents).expect("write fixture"))
        }

        fn group(&self, name: &str, contents: &str) -> PathBuf {
            let dir = self.0.join(crate::store::file::GROUPS_DIR);
            self.document(&dir, &format!("{name}.toml"))
                .tap(|p| fs::write(p, contents).expect("write group fixture"))
        }

        /// `<stem>.<ext>` under `parent` becomes `<parent>/<stem>/<canonical>.<ext>`.
        fn document(&self, parent: &Path, name: &str) -> PathBuf {
            let named = Path::new(name);
            let (identity, file) = match (named.file_stem(), named.extension()) {
                (Some(stem), Some(ext)) => {
                    let ext = ext.to_str().expect("utf-8 extension");
                    (
                        stem.to_str().expect("utf-8 stem").to_string(),
                        format!("{}.{ext}", crate::format::canonical_stem(ext)),
                    )
                }
                // No extension at all: the whole name is the identity, and the document
                // inside carries it too — so "this is not a candidate" still holds for
                // the same reason it did.
                _ => (name.to_string(), name.to_string()),
            };
            let dir = parent.join(identity);
            fs::create_dir_all(&dir).expect("create identity dir");
            dir.join(file)
        }
    }

    /// Do something with a value and hand it back. Keeps a fixture one expression.
    trait Tap: Sized {
        fn tap(self, f: impl FnOnce(&Self)) -> Self {
            f(&self);
            self
        }
    }
    impl Tap for PathBuf {}

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A body shaped like what the installer actually POSTs.
    fn body_with(mac: &str) -> String {
        format!(
            r#"{{"product":"PowerEdge R620","network_interfaces":[
                 {{"name":"eno1","mac":"{mac}","link":true}},
                 {{"name":"eno2","mac":"aa:bb:cc:00:11:22","link":false}}],
               "disks":[{{"path":"/dev/sda","size":500107862016}}]}}"#
        )
    }

    use crate::facts::Facts;
    use toml_edit::DocumentMut;

    fn facts_for(mac: &str) -> Facts {
        Facts::new(None, body_with(mac).as_bytes())
    }

    fn resolve(dir: &Path, mac: &str) -> Option<Resolution> {
        Answers::from_dir(dir)
            .resolve(&facts_for(mac))
            .expect("resolve should not fail")
    }

    // ---- normalization -----------------------------------------------------

    #[test]
    fn normalize_strips_separators_and_lowercases() {
        assert_eq!(normalize(b"98-FA-9b:50.d8 10"), "98fa9b50d810");
        assert_eq!(normalize(b""), "");
        assert_eq!(normalize(b"---"), "");
    }

    #[test]
    fn normalize_drops_non_ascii_without_panicking() {
        assert_eq!(normalize("clé-98FA".as_bytes()), "cl98fa");
        assert_eq!(normalize(&[0xff, b'a', 0x80, b'B']), "ab");
    }

    // ---- machine files -----------------------------------------------------

    #[test]
    fn matches_regardless_of_separator_style() {
        for filename in [
            "98-fa-9b-50-d8-10.toml",
            "98fa9b50d810.toml",
            "98:fa:9b:50:d8:10.toml",
        ] {
            let dir = TempDir::new();
            dir.write(filename, "[global]\nkeyboard = \"fr\"\n");
            let r = resolve(dir.path(), "98:fa:9b:50:d8:10")
                .unwrap_or_else(|| panic!("{filename}: expected a match"));
            assert!(r.body.contains("keyboard"), "{filename}: {}", r.body);
            assert!(r.machine.is_some(), "{filename}");
        }
    }

    #[test]
    fn matches_with_mixed_case_on_both_sides() {
        let dir = TempDir::new();
        dir.write("98-FA-9B-50-D8-10.toml", "[global]\nkeyboard = \"fr\"\n");
        assert!(resolve(dir.path(), "98:fa:9b:50:d8:10").is_some());
    }

    #[test]
    fn falls_back_to_default_when_nothing_matches() {
        let dir = TempDir::new();
        dir.write("98-fa-9b-50-d8-10.toml", "[global]\nkeyboard = \"fr\"\n");
        dir.write("default.toml", "[global]\nkeyboard = \"us\"\n");
        let r = resolve(dir.path(), "11:22:33:44:55:66").expect("default applies");
        assert!(r.used_default);
        assert!(r.body.contains("\"us\""), "{}", r.body);
    }

    #[test]
    fn no_match_and_no_default_yields_nothing() {
        let dir = TempDir::new();
        dir.write("98-fa-9b-50-d8-10.toml", "[global]\nx = 1\n");
        assert!(resolve(dir.path(), "11:22:33:44:55:66").is_none());
    }

    #[test]
    fn empty_and_missing_directories_yield_nothing() {
        let dir = TempDir::new();
        assert!(resolve(dir.path(), "98:fa:9b:50:d8:10").is_none());
        let missing = dir.path().join("nope");
        assert!(
            Answers::from_dir(&missing)
                .resolve(&Facts::new(None, b"anything"))
                .expect("a missing directory is not an error")
                .is_none()
        );
    }

    #[test]
    fn ambiguous_matches_resolve_in_sorted_order() {
        let dir = TempDir::new();
        dir.write("aabbccddeeff.toml", "[g]\npick = \"second\"\n");
        dir.write("001122334455.toml", "[g]\npick = \"first\"\n");
        let body = br#"{"macs":["00:11:22:33:44:55","aa:bb:cc:dd:ee:ff"]}"#;
        for _ in 0..5 {
            let r = Answers::from_dir(dir.path())
                .resolve(&Facts::new(None, body))
                .unwrap()
                .unwrap();
            assert_eq!(r.machine.as_deref(), Some("001122334455"));
        }
    }

    #[test]
    fn stem_of_only_punctuation_never_matches_everything() {
        let dir = TempDir::new();
        dir.write("--.toml", "[g]\nx = 1\n");
        assert!(resolve(dir.path(), "98:fa:9b:50:d8:10").is_none());
    }

    #[test]
    fn ignores_unservable_files_and_subdirectories() {
        let dir = TempDir::new();
        // Extensions outside the allowlist, so a stray note never answers a request.
        dir.write("98-fa-9b-50-d8-10.txt", "wrong extension");
        dir.write("98fa9b50d810.md", "wrong extension");
        dir.write("98fa9b50d810", "no extension");
        fs::create_dir(dir.path().join("98fa9b50d810.toml")).expect("create dir");
        assert!(resolve(dir.path(), "98:fa:9b:50:d8:10").is_none());
    }

    #[test]
    fn a_machine_can_be_answered_in_any_supported_format() {
        for (name, body, expected) in [
            ("98fa9b50d810.toml", "marker = \"toml\"\n", "toml"),
            ("98fa9b50d810.yaml", "marker: yaml\n", "yaml"),
            ("98fa9b50d810.json", "{\"marker\":\"json\"}", "json"),
            ("98fa9b50d810.xml", "<r><marker>xml</marker></r>", "xml"),
            ("98fa9b50d810.ks", "# kickstart\nlang en_US\n", "en_US"),
            (
                "98fa9b50d810.preseed",
                "d-i debian-installer/locale string en_US\n",
                "locale",
            ),
        ] {
            let dir = TempDir::new();
            dir.write(name, body);
            let r = resolve(dir.path(), "98:fa:9b:50:d8:10")
                .unwrap_or_else(|| panic!("{name}: no answer"));
            assert!(r.body.contains(expected), "{name}: {}", r.body);
        }
    }

    #[test]
    fn the_groups_directory_is_never_matched_as_a_machine() {
        // `groups` normalizes to "groups", which could in principle appear in a body.
        let dir = TempDir::new();
        dir.group("rack-a", "[global]\nkeyboard = \"fr\"\n");
        let body = br#"{"note":"this body mentions groups explicitly"}"#;
        assert!(
            Answers::from_dir(dir.path())
                .resolve(&Facts::new(None, body))
                .unwrap()
                .is_none()
        );
    }

    // ---- groups ------------------------------------------------------------

    #[test]
    fn a_group_serves_its_members() {
        let dir = TempDir::new();
        dir.group(
            "rack-a",
            "members = [\"98:fa:9b:50:d8:10\", \"aa:bb:cc:dd:ee:ff\"]\n\
             [global]\ncountry = \"fr\"\n",
        );
        for mac in ["98:fa:9b:50:d8:10", "aa:bb:cc:dd:ee:ff"] {
            let r = resolve(dir.path(), mac).unwrap_or_else(|| panic!("{mac} is a member"));
            assert_eq!(r.group.as_deref(), Some("rack-a"));
            assert!(r.body.contains("country"), "{}", r.body);
            // The membership list is ours, not Proxmox's.
            assert!(!r.body.contains("members"), "{}", r.body);
        }
        // A machine that is not a member gets nothing.
        assert!(resolve(dir.path(), "11:22:33:44:55:66").is_none());
    }

    #[test]
    fn a_member_matches_whatever_separator_style_it_was_written_in() {
        let dir = TempDir::new();
        dir.group("rack-a", "members = [\"98fa9b50d810\"]\n[global]\nx = 1\n");
        assert!(resolve(dir.path(), "98:FA:9B:50:D8:10").is_some());
    }

    #[test]
    fn a_machine_file_layers_on_top_of_its_group() {
        let dir = TempDir::new();
        dir.group(
            "rack-a",
            "members = [\"98:fa:9b:50:d8:10\"]\n\
             [global]\ncountry = \"fr\"\nkeyboard = \"fr\"\n\
             [disk-setup]\nfilesystem = \"zfs\"\n",
        );
        dir.write(
            "98-fa-9b-50-d8-10.toml",
            "[global]\nkeyboard = \"us\"\n[disk-setup]\ndisk_list = [\"nvme0n1\"]\n",
        );

        let r = resolve(dir.path(), "98:fa:9b:50:d8:10").expect("both apply");
        assert_eq!(r.machine.as_deref(), Some("98-fa-9b-50-d8-10"));
        assert_eq!(r.group.as_deref(), Some("rack-a"));

        let doc: DocumentMut = r.body.parse().expect("served toml must be valid");
        // The machine wins where they disagree...
        assert_eq!(doc["global"]["keyboard"].as_str(), Some("us"));
        // ...the group survives where it does not...
        assert_eq!(doc["global"]["country"].as_str(), Some("fr"));
        assert_eq!(doc["disk-setup"]["filesystem"].as_str(), Some("zfs"));
        // ...and additions land.
        assert!(doc["disk-setup"]["disk_list"].is_array());
    }

    #[test]
    fn a_group_can_extend_another_group() {
        let dir = TempDir::new();
        dir.group(
            "base",
            "[global]\ncountry = \"fr\"\ntimezone = \"Europe/Paris\"\n",
        );
        dir.group(
            "rack-a",
            "extends = \"base\"\nmembers = [\"98:fa:9b:50:d8:10\"]\n\
             [global]\ntimezone = \"UTC\"\n[disk-setup]\nfilesystem = \"zfs\"\n",
        );

        let r = resolve(dir.path(), "98:fa:9b:50:d8:10").expect("member of rack-a");
        let doc: DocumentMut = r.body.parse().expect("valid toml");
        assert_eq!(doc["global"]["country"].as_str(), Some("fr")); // from base
        assert_eq!(doc["global"]["timezone"].as_str(), Some("UTC")); // child wins
        assert_eq!(doc["disk-setup"]["filesystem"].as_str(), Some("zfs"));
        assert!(!r.body.contains("extends"), "{}", r.body);
    }

    #[test]
    fn a_machine_can_extend_a_group_it_is_not_a_member_of() {
        let dir = TempDir::new();
        dir.group("rack-a", "[global]\ncountry = \"fr\"\n");
        dir.write(
            "98-fa-9b-50-d8-10.toml",
            "extends = \"rack-a\"\n[global]\nkeyboard = \"us\"\n",
        );
        let r = resolve(dir.path(), "98:fa:9b:50:d8:10").expect("explicit extends");
        assert_eq!(r.group.as_deref(), Some("rack-a"));
        let doc: DocumentMut = r.body.parse().unwrap();
        assert_eq!(doc["global"]["country"].as_str(), Some("fr"));
    }

    #[test]
    fn an_explicit_extends_beats_membership() {
        let dir = TempDir::new();
        dir.group(
            "aaa-membership",
            "members = [\"98:fa:9b:50:d8:10\"]\n[g]\npick = \"member\"\n",
        );
        dir.group("zzz-explicit", "[g]\npick = \"explicit\"\n");
        dir.write("98-fa-9b-50-d8-10.toml", "extends = \"zzz-explicit\"\n");

        let r = resolve(dir.path(), "98:fa:9b:50:d8:10").unwrap();
        assert_eq!(r.group.as_deref(), Some("zzz-explicit"));
        let doc: DocumentMut = r.body.parse().unwrap();
        assert_eq!(doc["g"]["pick"].as_str(), Some("explicit"));
    }

    #[test]
    fn default_toml_may_extend_a_group_too() {
        let dir = TempDir::new();
        dir.group("base", "[global]\ncountry = \"fr\"\n");
        dir.write(
            "default.toml",
            "extends = \"base\"\n[global]\nkeyboard = \"us\"\n",
        );
        let r = resolve(dir.path(), "11:22:33:44:55:66").expect("default applies");
        assert!(r.used_default);
        let doc: DocumentMut = r.body.parse().unwrap();
        assert_eq!(doc["global"]["country"].as_str(), Some("fr"));
        assert_eq!(doc["global"]["keyboard"].as_str(), Some("us"));
    }

    // ---- misconfiguration --------------------------------------------------

    #[test]
    fn extending_an_unknown_group_fails_loudly() {
        // Serving a config whose base is missing would install a machine half-built.
        let dir = TempDir::new();
        dir.write(
            "98-fa-9b-50-d8-10.toml",
            "extends = \"nope\"\n[global]\nx = 1\n",
        );
        let err = Answers::from_dir(dir.path())
            .resolve(&facts_for("98:fa:9b:50:d8:10"))
            .expect_err("must not serve a half-built answer");
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[test]
    fn a_group_cycle_is_reported_and_the_group_is_dropped() {
        let dir = TempDir::new();
        dir.group(
            "a",
            "extends = \"b\"\nmembers = [\"98:fa:9b:50:d8:10\"]\n[g]\nx = 1\n",
        );
        dir.group("b", "extends = \"a\"\n[g]\ny = 2\n");

        let answers = Answers::from_dir(dir.path());
        let problems = answers.problems().unwrap();
        assert!(
            problems.iter().any(|p| p.contains("cycle")),
            "expected a cycle to be reported, got {problems:?}"
        );
        // The broken groups are not served.
        assert!(answers.group_names().unwrap().is_empty());
    }

    #[test]
    fn a_group_extending_a_missing_parent_is_reported() {
        let dir = TempDir::new();
        dir.group(
            "rack-a",
            "extends = \"ghost\"\nmembers = [\"98:fa:9b:50:d8:10\"]\n",
        );
        let answers = Answers::from_dir(dir.path());
        assert!(
            answers
                .problems()
                .unwrap()
                .iter()
                .any(|p| p.contains("ghost")),
            "{:?}",
            answers.problems().unwrap()
        );
    }

    #[test]
    fn an_invalid_group_file_does_not_break_the_other_groups() {
        let dir = TempDir::new();
        dir.group("broken", "this is not = = toml\n");
        dir.group(
            "rack-a",
            "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nx = 1\n",
        );

        let answers = Answers::from_dir(dir.path());
        assert!(!answers.problems().unwrap().is_empty());
        // The healthy rack still installs.
        let r = answers
            .resolve(&facts_for("98:fa:9b:50:d8:10"))
            .unwrap()
            .expect("rack-a still works");
        assert_eq!(r.group.as_deref(), Some("rack-a"));
    }

    #[test]
    fn an_invalid_machine_file_is_an_error_not_a_silent_wrong_answer() {
        let dir = TempDir::new();
        dir.write("98-fa-9b-50-d8-10.toml", "not = = valid\n");
        assert!(
            Answers::from_dir(dir.path())
                .resolve(&facts_for("98:fa:9b:50:d8:10"))
                .is_err()
        );
    }

    // ---- caching -----------------------------------------------------------

    #[test]
    fn a_new_file_is_picked_up_without_restarting() {
        // One instance across the change: this is what exercises cache invalidation.
        let dir = TempDir::new();
        let answers = Answers::from_dir(dir.path());
        let body = body_with("98:fa:9b:50:d8:10");

        assert!(
            answers
                .resolve(&Facts::new(None, body.as_bytes()))
                .unwrap()
                .is_none()
        );
        dir.write("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n");
        assert!(
            answers
                .resolve(&Facts::new(None, body.as_bytes()))
                .unwrap()
                .is_some(),
            "a file added after the first request must be picked up"
        );
    }

    #[test]
    fn a_removed_machine_stops_being_served() {
        let dir = TempDir::new();
        let answers = Answers::from_dir(dir.path());
        let path = dir.write("98fa9b50d810.toml", "[global]\nx = 1\n");
        let body = body_with("98:fa:9b:50:d8:10");

        assert!(
            answers
                .resolve(&Facts::new(None, body.as_bytes()))
                .unwrap()
                .is_some()
        );
        // The whole identity, which is what removing a machine now means. Its directory
        // leaving the answers directory moves that directory's mtime, so this is picked
        // up at once rather than at the backstop.
        fs::remove_dir_all(path.parent().unwrap()).expect("remove fixture");
        assert!(
            answers
                .resolve(&Facts::new(None, body.as_bytes()))
                .unwrap()
                .is_none()
        );
    }

    /// One document leaving a machine that keeps others — the case the mtime cannot see.
    ///
    /// A directory's mtime moves when an entry is added or removed *in it*, and this
    /// happens one level down, inside the machine's own directory. So the answers
    /// directory looks untouched and only `RELOAD_BACKSTOP` notices, exactly as it
    /// already did for a file edited in place. Worth a slow test: the alternative is a
    /// stat per machine on every request, which is what the cache exists to avoid.
    #[test]
    fn a_document_removed_from_a_machine_stops_being_served_within_the_backstop() {
        let dir = TempDir::new();
        let answers = Answers::from_dir(dir.path());
        let path = dir.write("98fa9b50d810.toml", "[global]\nx = 1\n");
        dir.write("98fa9b50d810.ipxe", "#!ipxe\nchain x\n");
        let body = body_with("98:fa:9b:50:d8:10");
        // Asked for on the Proxmox endpoint, so the machine's `.ipxe` cannot stand in
        // for the document that was removed.
        let toml = || Facts::from_request(Some("/proxmox/answer"), None, body.as_bytes());

        assert!(answers.resolve(&toml()).unwrap().is_some());
        fs::remove_file(&path).expect("remove fixture");

        // Poll rather than sleep exactly once, so a slow machine does not make this
        // flaky and a fast one does not make it slow.
        let deadline = Instant::now() + RELOAD_BACKSTOP * 3;
        while Instant::now() < deadline {
            if answers.resolve(&toml()).unwrap().is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the removed document was still being served after the backstop");
    }

    /// The cache hides a write made *inside* an existing machine's directory — which is
    /// exactly what `admin::guarded` compares across.
    ///
    /// Without the invalidation the guard snapshots `problems()`, writes, reads the same
    /// cached answer back, sees no difference and keeps a write that broke the answer set.
    /// Over SQLite that cannot happen (its `version` is an atomic bumped by every write);
    /// over files the version is the answers directory's mtime, and nothing was added or
    /// removed *in it*.
    ///
    /// **Watched failing:** empty the body of `Answers::invalidate` and the last assertion
    /// goes red while the middle one stays green.
    #[test]
    fn a_write_inside_an_identity_directory_is_invisible_until_the_cache_is_dropped() {
        let dir = TempDir::new();
        let answers = Answers::from_dir(dir.path());
        dir.write("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n");

        // Populates the cache, and is the state `guarded` would keep as `before`.
        assert!(answers.problems().expect("problems").is_empty());

        // A second document of the same format in the same directory is a reported
        // problem — and one level down, so the mtime the cache watches does not move.
        fs::write(
            dir.path().join("98fa9b50d810").join("second.toml"),
            "[global]\nkeyboard = \"us\"\n",
        )
        .expect("write the conflicting document");

        // Asserted rather than assumed: if the cache ever stops hiding this, the
        // invalidation below is dead weight and someone should find that out here.
        assert!(
            answers.problems().expect("problems").is_empty(),
            "expected the cached listing to hide a write one level down"
        );

        answers.invalidate();
        let problems = answers.problems().expect("problems");
        assert!(
            !problems.is_empty(),
            "dropping the cache must reveal the conflict, got {problems:?}"
        );
    }

    #[test]
    fn repeated_lookups_are_consistent() {
        let dir = TempDir::new();
        let answers = Answers::from_dir(dir.path());
        dir.write("98fa9b50d810.toml", "[global]\nkeyboard = \"fr\"\n");
        dir.write("default.toml", "[global]\nkeyboard = \"us\"\n");
        let hit = body_with("98:fa:9b:50:d8:10");
        let miss = body_with("11:22:33:44:55:66");

        for _ in 0..20 {
            assert_eq!(
                answers
                    .resolve(&Facts::new(None, hit.as_bytes()))
                    .unwrap()
                    .unwrap()
                    .machine
                    .as_deref(),
                Some("98fa9b50d810")
            );
            assert!(
                answers
                    .resolve(&Facts::new(None, miss.as_bytes()))
                    .unwrap()
                    .unwrap()
                    .used_default
            );
        }
    }
}
