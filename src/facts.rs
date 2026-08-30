//! What we know about the machine that is asking.
//!
//! Three sources, deliberately layered from most to least structured:
//!
//! 1. **Query parameters** — `?mac=…&uuid=…&serial=…`. This is how every installer
//!    other than Proxmox identifies itself: iPXE substitutes the values into the URL
//!    before fetching. Reliable, arbitrary keys, no guessing.
//! 2. **The POST body, when it is JSON** — flattened to both its full dotted paths
//!    (`dmi.system.serial`) and its bare leaf names (`serial`). Matching on a leaf name
//!    is the point: Proxmox's own documentation warns that the contents of `dmi` "might
//!    vary wildly, depending on the system", so a selector that says "a field called
//!    `serial`, wherever it lives" survives a reorganisation that a fixed path would
//!    not. This is the departure from the original spec's "do not parse the JSON", and
//!    it is why the leaf-name form exists at all.
//! 3. **The raw body**, normalized to lowercase alphanumerics — the original substring
//!    haystack, kept exactly as it was so that matching by filename still works.

use std::collections::BTreeMap;

/// Everything a request tells us about the machine.
#[derive(Debug, Default, Clone)]
pub struct Facts {
    /// Label → every value seen for it. A machine has several MACs; a leaf name like
    /// `serial` can occur in more than one place in the DMI tree.
    labels: BTreeMap<String, Vec<String>>,
    /// The whole body, lowercased and stripped to alphanumerics.
    haystack: String,
}

impl Facts {
    pub fn new(query: Option<&str>, body: &[u8]) -> Facts {
        Facts::from_request(None, query, body)
    }

    /// Everything a request carries, path included.
    ///
    /// The path is not decoration. cloud-init's NoCloud datasource fetches several
    /// *named* files from one URL — `user-data` and `meta-data` are both required, and
    /// it skips the datasource entirely if either is missing — so the same server has to
    /// answer them differently. NoCloud can also expand `__dmi.chassis-serial-number__`
    /// into the URL, which puts a machine's identity in the path rather than the query.
    pub fn from_request(path: Option<&str>, query: Option<&str>, body: &[u8]) -> Facts {
        let mut facts = Facts {
            haystack: crate::select::normalize(body),
            ..Default::default()
        };

        if let Some(path) = path {
            let trimmed = path.trim_matches('/');
            if !trimmed.is_empty() {
                facts.add("path", trimmed.to_string());
                let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
                if let Some(last) = segments.last() {
                    facts.add("file", (*last).to_string());
                }
                for segment in &segments {
                    facts.add("segment", (*segment).to_string());
                    // Into the haystack too: with NoCloud's DMI expansion the serial
                    // arrives as a path segment, and a document named after it must
                    // still resolve.
                    facts
                        .haystack
                        .push_str(&crate::select::normalize(segment.as_bytes()));
                }
            }
        }

        if let Some(query) = query {
            for (key, value) in parse_query(query) {
                // Also into the haystack: a document named after a MAC must resolve
                // whether that MAC arrived in a POST body or a query string. Identity
                // matching should not care how the machine introduced itself.
                facts
                    .haystack
                    .push_str(&crate::select::normalize(value.as_bytes()));
                facts.add(&key, value);
            }
        }

        // Only if it really is JSON. A body that is not stays a haystack and nothing
        // more, which is the pre-existing behaviour.
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
            flatten(&value, &mut String::new(), &mut facts);
        }

        facts
    }

    /// An identifier and nothing else — what `render <mac>` and `GET /resolve/<id>`
    /// have to work with. It fills the haystack, so documents that match by name still
    /// resolve, but claims nothing about *what kind* of identifier it is.
    pub fn from_identity(id: &str) -> Facts {
        Facts {
            haystack: crate::select::normalize(id.as_bytes()),
            ..Default::default()
        }
    }

    /// Facts from labels alone, for tests and for `render --query`.
    pub fn from_labels(pairs: &[(&str, &str)]) -> Facts {
        let mut facts = Facts::default();
        for (k, v) in pairs {
            facts.add(k, (*v).to_string());
            // A bare identifier is also its own haystack, so `render <mac>` keeps
            // working against documents that match by filename.
            facts
                .haystack
                .push_str(&crate::select::normalize(v.as_bytes()));
        }
        facts
    }

    fn add(&mut self, key: &str, value: String) {
        if key.is_empty() || value.is_empty() {
            return;
        }
        let entry = self.labels.entry(key.to_ascii_lowercase()).or_default();
        if !entry.contains(&value) {
            entry.push(value);
        }
    }

    pub fn haystack(&self) -> &str {
        &self.haystack
    }

    pub fn values(&self, key: &str) -> &[String] {
        self.labels
            .get(&key.to_ascii_lowercase())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Does any value for `key` match `pattern`?
    ///
    /// Comparison is normalized the same way filenames are — lowercase, punctuation
    /// dropped — so `98:fa:9b:50:d8:10`, `98-fa-9b-50-d8-10` and `98fa9b50d810` are the
    /// same MAC on both sides, and case never matters for a serial.
    pub fn matches(&self, key: &str, pattern: &str) -> bool {
        let wanted = normalize_pattern(pattern);
        self.values(key)
            .iter()
            .any(|v| glob(&wanted, &crate::select::normalize(v.as_bytes())))
    }

    /// Every label known, for diagnostics.
    /// Whatever this request said about which machine it is, for a log line.
    ///
    /// **This exists because a 404 named no machine**, and that made the single most
    /// valuable thing a dashboard could show — *these machines are asking and I have no
    /// answer for them* — impossible to derive. For a GET the identity is in the query
    /// string, which is already in the logged target; for a **Proxmox POST it is only in
    /// the body**, and the body is not logged.
    ///
    /// It is logging, not instrumentation: no counter, no state, no memory, one `format!`
    /// on a path that is already failing.
    pub fn identity(&self) -> Option<String> {
        // In the order a person would want them, and only the labels that actually name a
        // machine — never the whole flattened body, which would put a password hash in
        // the log.
        for key in ["mac", "macaddress", "serial", "uuid", "product", "fqdn"] {
            if let Some(values) = self.labels.get(key)
                && let Some(first) = values.iter().find(|v| !v.is_empty())
            {
                return Some(format!("{key}={first}"));
            }
        }
        None
    }

    pub fn labels(&self) -> impl Iterator<Item = (&String, &Vec<String>)> {
        self.labels.iter()
    }
}

/// Normalize a selector pattern the way values are normalized — but keeping `*` and
/// `?`, which the ordinary normalization would strip along with the punctuation. Losing
/// them silently turns every glob into a literal, which is a quiet and confusing
/// failure rather than a loud one.
pub fn normalize_pattern(pattern: &str) -> String {
    pattern
        .bytes()
        .filter(|b| b.is_ascii_alphanumeric() || *b == b'*' || *b == b'?')
        .map(|b| b.to_ascii_lowercase() as char)
        .collect()
}

/// `a=1&b=2`, with percent-decoding. Hand-rolled rather than pulling in a URL crate for
/// twenty lines of work.
fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (percent_decode(k), percent_decode(v)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&input[i + 1..i + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    // Not a real escape: keep the '%' as written rather than guessing.
                    Err(_) => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Walk a JSON document, recording every scalar under both its full path and its bare
/// leaf name. Array indices become part of the path but not of the leaf name, so
/// `network_interfaces.0.mac` is also reachable as plain `mac`.
fn flatten(value: &serde_json::Value, path: &mut String, facts: &mut Facts) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let restore = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(key);
                flatten(child, path, facts);
                path.truncate(restore);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let restore = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(&index.to_string());
                flatten(child, path, facts);
                path.truncate(restore);
            }
        }
        scalar => {
            let text = match scalar {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => return,
                other => other.to_string(),
            };
            let path_key = path.clone();
            // The leaf name is the last path segment that is not an array index.
            let leaf = path
                .rsplit('.')
                .find(|segment| segment.parse::<usize>().is_err())
                .unwrap_or("")
                .to_string();
            facts.add(&path_key, text.clone());
            if !leaf.is_empty() && leaf != path_key {
                facts.add(&leaf, text);
            }
        }
    }
}

/// `*` matches any run of characters, `?` exactly one. Both sides arrive normalized, so
/// this never has to think about separators or case.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();

    // Iterative backtracking: no recursion, so a pathological pattern cannot blow the
    // stack of a thread serving an install.
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut resume) = (None, 0usize);

    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            resume = ti;
            pi += 1;
        } else if let Some(s) = star {
            // Backtrack: let the last '*' swallow one more character.
            pi = s + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxmox_body() -> &'static str {
        // Shaped after what the installer actually posts: a schema marker, product and
        // ISO blocks, DMI carrying serials and a UUID, and the interface list.
        r#"{
          "$schema": "https://…/device-info-schema-1.0.json",
          "product": { "fullname": "Proxmox VE", "short": "pve" },
          "iso": { "release": "8.2", "isorelease": "1" },
          "dmi": {
            "system": {
              "manufacturer": "Dell Inc.",
              "product": "PowerEdge R620",
              "serial": "7ABC123",
              "uuid": "4c4c4544-0037-4110-8043-b7c04f313233",
              "sku": "SKU=NotProvided"
            },
            "baseboard": { "serial": "..CN7016360J0123.", "asset-tag": "rack-12" },
            "chassis": { "serial": "7ABC123", "asset-tag": "row-4" }
          },
          "network_interfaces": [
            { "index": 0, "name": "eno1", "mac": "98:fa:9b:50:d8:10", "link": true },
            { "index": 1, "name": "eno2", "mac": "aa:bb:cc:00:11:22", "link": false }
          ],
          "disks": [ { "path": "/dev/sda", "size": 500107862016 } ]
        }"#
    }

    // ---- query parameters --------------------------------------------------

    #[test]
    fn query_parameters_become_labels() {
        let f = Facts::new(Some("mac=52:54:00:89:d8:10&uuid=abc-123&serial=X1"), b"");
        assert!(f.matches("mac", "52:54:00:89:d8:10"));
        assert!(f.matches("uuid", "abc-123"));
        assert!(f.matches("serial", "X1"));
        assert!(!f.matches("serial", "X2"));
    }

    #[test]
    fn query_parameters_are_percent_decoded() {
        let f = Facts::new(Some("serial=A%2FB&name=web+01&pad=%zz"), b"");
        assert_eq!(f.values("serial"), ["A/B"]);
        assert_eq!(f.values("name"), ["web 01"]);
        // A malformed escape is kept verbatim rather than guessed at.
        assert_eq!(f.values("pad"), ["%zz"]);
    }

    #[test]
    fn a_query_value_also_reaches_the_haystack() {
        // Otherwise a GET carrying `?mac=…` could never match a file named after it,
        // because a GET has no body to search.
        let f = Facts::new(Some("mac=98:fa:9b:50:d8:10"), b"");
        assert!(f.haystack().contains("98fa9b50d810"), "{}", f.haystack());
    }

    #[test]
    fn a_missing_or_empty_query_is_not_an_error() {
        assert!(Facts::new(None, b"").values("mac").is_empty());
        assert!(Facts::new(Some(""), b"").values("mac").is_empty());
        assert!(Facts::new(Some("&&="), b"").values("mac").is_empty());
    }

    // ---- the JSON body -----------------------------------------------------

    #[test]
    fn a_leaf_name_matches_wherever_it_lives_in_the_tree() {
        // The whole point: `dmi` may be reorganised between Proxmox versions, and a
        // selector written against a leaf name survives that.
        let f = Facts::new(None, proxmox_body().as_bytes());
        assert!(f.matches("serial", "7ABC123"));
        assert!(f.matches("uuid", "4c4c4544-0037-4110-8043-b7c04f313233"));
        assert!(f.matches("manufacturer", "Dell Inc."));
        assert!(f.matches("product", "PowerEdge R620"));
    }

    #[test]
    fn the_full_path_is_available_when_precision_is_wanted() {
        let f = Facts::new(None, proxmox_body().as_bytes());
        assert!(f.matches("dmi.system.serial", "7ABC123"));
        assert!(f.matches("dmi.chassis.asset-tag", "row-4"));
        assert!(f.matches("network_interfaces.0.mac", "98:fa:9b:50:d8:10"));
        // Precision cuts both ways, which is the trade the operator is making.
        assert!(!f.matches("dmi.system.asset-tag", "row-4"));
    }

    #[test]
    fn every_value_of_a_repeated_leaf_is_kept() {
        let f = Facts::new(None, proxmox_body().as_bytes());
        // Both interfaces' MACs must be matchable, not just the first.
        assert!(f.matches("mac", "98:fa:9b:50:d8:10"));
        assert!(f.matches("mac", "aa:bb:cc:00:11:22"));
        // `serial` occurs under system, baseboard and chassis.
        assert!(f.values("serial").len() >= 2, "{:?}", f.values("serial"));
    }

    #[test]
    fn separator_style_and_case_never_matter() {
        let f = Facts::new(None, proxmox_body().as_bytes());
        for written in ["98:fa:9b:50:d8:10", "98-FA-9B-50-D8-10", "98fa9b50d810"] {
            assert!(f.matches("mac", written), "{written} should match");
        }
    }

    #[test]
    fn numbers_and_booleans_are_matchable_as_written() {
        let f = Facts::new(None, proxmox_body().as_bytes());
        assert!(f.matches("size", "500107862016"));
        assert!(f.matches("link", "true"));
    }

    #[test]
    fn a_body_that_is_not_json_still_yields_a_haystack() {
        // Nothing may regress for a request shape we did not anticipate.
        let f = Facts::new(None, b"not json at all 98:FA:9B:50:D8:10");
        assert!(f.haystack().contains("98fa9b50d810"));
        assert!(f.values("mac").is_empty());
    }

    #[test]
    fn the_haystack_still_holds_the_whole_body() {
        let f = Facts::new(None, proxmox_body().as_bytes());
        assert!(f.haystack().contains("98fa9b50d810"));
        assert!(f.haystack().contains("7abc123"));
    }

    #[test]
    fn malformed_json_does_not_panic() {
        for body in [
            b"{" as &[u8],
            b"{\"a\":}",
            b"[[[[[[[[[[",
            b"\xff\xfe binary",
            b"",
        ] {
            let _ = Facts::new(None, body);
        }
    }

    // ---- the request path --------------------------------------------------

    #[test]
    fn the_path_is_available_whole_and_in_pieces() {
        let f = Facts::from_request(Some("/ubuntu/user-data"), None, b"");
        assert!(f.matches("path", "ubuntu/user-data"));
        assert!(f.matches("file", "user-data"));
        assert!(f.matches("segment", "ubuntu"));
        assert!(f.matches("segment", "user-data"));
        assert!(!f.matches("file", "meta-data"));
    }

    #[test]
    fn the_two_nocloud_files_can_be_told_apart() {
        // cloud-init requires both, and skips the datasource if either is missing —
        // so answering them identically breaks every Ubuntu install.
        let user = Facts::from_request(Some("/user-data"), None, b"");
        let meta = Facts::from_request(Some("/meta-data"), None, b"");
        assert!(user.matches("file", "user-data") && !user.matches("file", "meta-data"));
        assert!(meta.matches("file", "meta-data") && !meta.matches("file", "user-data"));
    }

    #[test]
    fn an_identity_in_the_path_still_finds_its_document() {
        // NoCloud expands `__dmi.chassis-serial-number__` into the URL.
        let f = Facts::from_request(Some("/7ABC123/user-data"), None, b"");
        assert!(f.haystack().contains("7abc123"), "{}", f.haystack());
        assert!(f.matches("segment", "7ABC123"));
    }

    #[test]
    fn a_bare_or_missing_path_is_not_an_error() {
        for path in [None, Some(""), Some("/"), Some("///")] {
            let f = Facts::from_request(path, None, b"");
            assert!(f.values("path").is_empty(), "{path:?}");
            assert!(f.values("file").is_empty(), "{path:?}");
        }
    }

    #[test]
    fn a_path_glob_works_like_any_other() {
        let f = Facts::from_request(Some("/ubuntu/22.04/user-data"), None, b"");
        assert!(f.matches("path", "ubuntu/*"));
        assert!(f.matches("file", "user*"));
    }

    // ---- globs -------------------------------------------------------------

    #[test]
    fn globs_match_what_they_should() {
        assert!(glob("7abc*", "7abc123"));
        assert!(glob("*123", "7abc123"));
        assert!(glob("7*3", "7abc123"));
        assert!(glob("7abc123", "7abc123"));
        assert!(glob("*", "anything"));
        assert!(glob("7abc???", "7abc123"));
        assert!(glob("", ""));
        assert!(glob("**", "x"));
    }

    #[test]
    fn globs_refuse_what_they_should() {
        assert!(!glob("7abc*", "6abc123"));
        assert!(!glob("*123", "7abc124"));
        assert!(!glob("7abc??", "7abc123"));
        assert!(!glob("", "x"));
        assert!(!glob("abc", ""));
    }

    #[test]
    fn a_pathological_glob_terminates() {
        // Backtracking is iterative on purpose: this must not blow a thread's stack or
        // hang a request.
        let pattern = "*a*a*a*a*a*a*a*a*b";
        let text = "a".repeat(200);
        assert!(!glob(pattern, &text));
    }

    #[test]
    fn a_pattern_keeps_its_glob_characters_through_normalization() {
        // The ordinary normalization strips everything non-alphanumeric, which would
        // quietly turn `7abc*` into the literal `7abc`.
        assert_eq!(normalize_pattern("7ABC-*"), "7abc*");
        assert_eq!(normalize_pattern("98:fa:9b:*"), "98fa9b*");
        assert_eq!(normalize_pattern("a?c"), "a?c");
    }

    #[test]
    fn globs_work_through_matches() {
        let f = Facts::new(None, proxmox_body().as_bytes());
        assert!(f.matches("serial", "7abc*"));
        assert!(f.matches("mac", "98fa9b*"));
        assert!(!f.matches("serial", "9xyz*"));
    }
}
