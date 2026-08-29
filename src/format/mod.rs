//! Answer documents, whatever they are written in.
//!
//! Every format supports the same three operations — parse, layer one over another,
//! render — so the machinery above (`select.rs`) never has to know which OS it is
//! serving. What differs is what "layer over" can honestly mean:
//!
//! * **TOML, YAML, JSON** merge structurally: maps merge key by key, arrays replace,
//!   anything else is replaced by the higher layer.
//! * **XML** merges structurally too, by element name and discriminating attribute —
//!   see `xml.rs` for exactly what that can and cannot promise.
//! * **Anything else** — kickstart, preseed, plain text — is **concatenated** in layer
//!   order. That is not a merge and this module does not pretend otherwise: a directive
//!   in a later layer does not remove an earlier one, it only follows it. Whether that
//!   ends up meaning "override" is the target format's business (preseed's last answer
//!   wins; kickstart's does not always).

pub mod xml;

use std::collections::BTreeMap;

/// Keys this server understands that no installer does. They steer resolution and must
/// never reach the machine.
pub const CONTROL_KEYS: [&str; 3] = ["extends", "members", "match"];

/// The element holding those keys in an XML document.
pub const XML_CONTROL_ELEMENT: &str = "answer-meta";

/// The marker introducing them in a line-oriented format.
pub const TEXT_DIRECTIVE: &str = "answer:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Toml,
    Yaml,
    Json,
    Xml,
    /// Line-oriented and opaque to us: kickstart, preseed, anything else.
    Text,
}

impl Kind {
    /// The formats we will pick up from a directory. Deliberately an allowlist: a
    /// stray `README.md` next to the answers must not become a candidate.
    pub fn for_extension(ext: &str) -> Option<Kind> {
        match ext.to_ascii_lowercase().as_str() {
            "toml" => Some(Kind::Toml),
            "yaml" | "yml" => Some(Kind::Yaml),
            "json" | "ign" => Some(Kind::Json),
            // `autoyast` and `unattend` are XML too. They exist as separate extensions
            // so that a store holding both a SUSE profile and a Windows unattend can
            // tell them apart — an endpoint asking for one must not receive the other.
            "xml" | "autoyast" | "unattend" => Some(Kind::Xml),
            // Kickstart, preseed, and the boot-script case. Deliberately not `txt`:
            // a stray notes file next to the answers must not become a candidate.
            "ks" | "cfg" | "preseed" | "seed" | "ipxe" => Some(Kind::Text),
            _ => None,
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            // Proxmox asks for TOML or JSON and is happy with text/plain.
            Kind::Toml => "text/plain; charset=utf-8",
            Kind::Yaml => "text/yaml; charset=utf-8",
            Kind::Json => "application/json",
            Kind::Xml => "application/xml; charset=utf-8",
            Kind::Text => "text/plain; charset=utf-8",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Toml => "toml",
            Kind::Yaml => "yaml",
            Kind::Json => "json",
            Kind::Xml => "xml",
            Kind::Text => "text",
        }
    }

    /// Whether layering is a real merge or a concatenation.
    pub fn merges_structurally(self) -> bool {
        self != Kind::Text
    }
}

/// Which document extensions an endpoint is asking for.
///
/// An installer fetching a URL expects one particular thing back — a kickstart client
/// wants kickstart, and would choke on TOML. That is not a convention anyone chose; it
/// is the protocol. So the endpoint declares the format, the document carries it as its
/// extension, and only documents of that format may answer.
///
/// This is deliberately **not** tied to how documents are stored. Directories and
/// database rows are a lookup space and must stay free to be reorganised; a URL is a
/// public contract baked into an ISO and must not move because someone renamed a folder.
///
/// A segment naming none of these constrains nothing, so a URL like `/answer` keeps
/// working exactly as it always has.
pub fn endpoint_formats(segment: &str) -> Option<&'static [&'static str]> {
    let segment = segment.to_ascii_lowercase();
    Some(match segment.as_str() {
        "proxmox" | "pve" | "toml" => &["toml"],
        // Deliberately not `seed`: `s=http://server/seed/` is a perfectly ordinary
        // NoCloud seed URL, which serves YAML. An alias has to be specific enough that
        // nobody reaches it by accident.
        "debian" | "preseed" => &["preseed", "seed"],
        "rhel" | "centos" | "fedora" | "alma" | "rocky" | "kickstart" | "ks" => &["ks"],
        "ubuntu" | "autoinstall" | "cloudinit" | "nocloud" | "yaml" | "yml" => &["yaml", "yml"],
        // `.autoyast` and `.unattend` disambiguate; plain `.xml` still answers either,
        // which is fine until a store holds both.
        "suse" | "opensuse" | "autoyast" => &["autoyast", "xml"],
        "windows" | "unattend" => &["unattend", "xml"],
        "flatcar" | "coreos" | "ignition" | "ign" => &["ign", "json"],
        "json" => &["json", "ign"],
        "xml" => &["xml"],
        "cfg" => &["cfg"],
        "ipxe" => &["ipxe"],
        _ => return None,
    })
}

/// The filename to write a new document under, before the extension.
///
/// **The stem decides nothing.** A document's format is its extension, and its identity
/// is the directory it sits in; `proxmox.toml` and `answer.toml` are the same document
/// to this server. This only picks a readable name for one nobody has named themselves
/// — a document already on disk keeps whatever it is called.
///
/// The names are the endpoint aliases wherever there is one, so a directory listing
/// reads the way the URLs do. `boot` is the exception and deliberately not an alias: an
/// `.ipxe` document is what boots the installer, not an operating system.
pub fn canonical_stem(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "toml" => "proxmox",
        "yaml" | "yml" => "ubuntu",
        "ign" => "flatcar",
        "autoyast" => "suse",
        "unattend" => "windows",
        "ks" => "rhel",
        "preseed" | "seed" => "debian",
        "ipxe" => "boot",
        // Nothing more specific to say: `json` is Ignition *or* a plain document, and
        // `xml`/`cfg` name no single installer. `answer.json` beats `json.json`, and an
        // extension we do not serve never reaches a write but is still owed a name.
        _ => "answer",
    }
}

#[derive(Debug, Clone)]
enum Inner {
    Toml(toml_edit::DocumentMut),
    Yaml(serde_yaml_ng::Value),
    Json(serde_json::Value),
    Xml(xml::Document),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct Doc {
    kind: Kind,
    inner: Inner,
}

/// What a document says about how it should be selected and composed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Control {
    pub extends: Option<String>,
    pub members: Vec<String>,
    /// Selector key → pattern.
    pub matchers: BTreeMap<String, String>,
}

impl Control {
    pub fn is_empty(&self) -> bool {
        self.extends.is_none() && self.members.is_empty() && self.matchers.is_empty()
    }
}

impl Doc {
    pub fn parse(kind: Kind, text: &str, origin: &str) -> Result<Doc, String> {
        let inner = match kind {
            Kind::Toml => Inner::Toml(
                text.parse::<toml_edit::DocumentMut>()
                    .map_err(|e| format!("{origin}: invalid TOML: {e}"))?,
            ),
            Kind::Yaml => Inner::Yaml(
                serde_yaml_ng::from_str(text)
                    .map_err(|e| format!("{origin}: invalid YAML: {e}"))?,
            ),
            Kind::Json => Inner::Json(
                serde_json::from_str(text).map_err(|e| format!("{origin}: invalid JSON: {e}"))?,
            ),
            Kind::Xml => {
                Inner::Xml(xml::parse(text).map_err(|e| format!("{origin}: invalid XML: {e}"))?)
            }
            Kind::Text => Inner::Text(text.to_string()),
        };
        Ok(Doc { kind, inner })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Layer `over` on top of this document.
    ///
    /// Refuses to mix formats: a YAML machine file cannot sensibly be laid over a TOML
    /// group, and quietly serving one of the two would be worse than saying so.
    pub fn merge(&mut self, over: &Doc) -> Result<(), String> {
        if self.kind != over.kind {
            return Err(format!(
                "cannot layer {} over {}: every layer of one answer must be the same format",
                over.kind.label(),
                self.kind.label()
            ));
        }
        match (&mut self.inner, &over.inner) {
            (Inner::Toml(base), Inner::Toml(o)) => crate::merge::merge_into(base, o),
            (Inner::Yaml(base), Inner::Yaml(o)) => merge_yaml(base, o),
            (Inner::Json(base), Inner::Json(o)) => merge_json(base, o),
            (Inner::Xml(base), Inner::Xml(o)) => xml::merge(&mut base.root, &o.root),
            (Inner::Text(base), Inner::Text(o)) => {
                // Concatenation, with a marker so the seam is visible to whoever has to
                // debug the result.
                if !base.ends_with('\n') && !base.is_empty() {
                    base.push('\n');
                }
                base.push_str(o);
            }
            _ => unreachable!("kinds were compared above"),
        }
        Ok(())
    }

    pub fn render(&self) -> String {
        match &self.inner {
            Inner::Toml(d) => d.to_string(),
            Inner::Yaml(v) => serde_yaml_ng::to_string(v).unwrap_or_default(),
            Inner::Json(v) => {
                let mut out = serde_json::to_string_pretty(v).unwrap_or_default();
                out.push('\n');
                out
            }
            Inner::Xml(d) => xml::render(d),
            Inner::Text(t) => t.clone(),
        }
    }

    /// Read this document's selection and composition keys.
    pub fn control(&self) -> Control {
        match &self.inner {
            Inner::Toml(d) => {
                let table = d.as_table();
                Control {
                    extends: table
                        .get("extends")
                        .and_then(|i| i.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    members: table
                        .get("members")
                        .and_then(|i| i.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str())
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                    matchers: table
                        .get("match")
                        .and_then(|i| i.as_table_like())
                        .map(|t| {
                            t.iter()
                                .filter_map(|(k, v)| {
                                    scalar_to_string_toml(v).map(|s| (k.to_string(), s))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                }
            }
            Inner::Yaml(v) => control_from_yaml(v),
            Inner::Json(v) => control_from_json(v),
            Inner::Xml(d) => control_from_xml(&d.root),
            Inner::Text(t) => control_from_text(t),
        }
    }

    /// Remove those keys, so what reaches the installer is only what it understands.
    pub fn strip_control(&mut self) {
        match &mut self.inner {
            Inner::Toml(d) => {
                let table = d.as_table_mut();
                for key in CONTROL_KEYS {
                    table.remove(key);
                }
            }
            Inner::Yaml(v) => {
                if let serde_yaml_ng::Value::Mapping(map) = v {
                    for key in CONTROL_KEYS {
                        map.remove(serde_yaml_ng::Value::String(key.to_string()));
                    }
                }
            }
            Inner::Json(v) => {
                if let serde_json::Value::Object(map) = v {
                    for key in CONTROL_KEYS {
                        map.remove(key);
                    }
                }
            }
            Inner::Xml(d) => {
                d.root.remove_children_named(XML_CONTROL_ELEMENT);
            }
            Inner::Text(t) => {
                *t = t
                    .lines()
                    .filter(|line| directive_of(line).is_none())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !t.is_empty() && !t.ends_with('\n') {
                    t.push('\n');
                }
            }
        }
    }
}

fn merge_yaml(base: &mut serde_yaml_ng::Value, over: &serde_yaml_ng::Value) {
    use serde_yaml_ng::Value as V;
    if let (V::Mapping(b), V::Mapping(o)) = (&mut *base, over) {
        for (key, value) in o {
            match b.get_mut(key) {
                Some(existing) => merge_yaml(existing, value),
                None => {
                    b.insert(key.clone(), value.clone());
                }
            }
        }
        return;
    }
    // Sequences and scalars alike: the higher layer wins outright.
    *base = over.clone();
}

fn merge_json(base: &mut serde_json::Value, over: &serde_json::Value) {
    use serde_json::Value as V;
    if let (V::Object(b), V::Object(o)) = (&mut *base, over) {
        for (key, value) in o {
            match b.get_mut(key) {
                Some(existing) => merge_json(existing, value),
                None => {
                    b.insert(key.clone(), value.clone());
                }
            }
        }
        return;
    }
    *base = over.clone();
}

fn scalar_to_string_toml(item: &toml_edit::Item) -> Option<String> {
    let value = item.as_value()?;
    Some(match value {
        toml_edit::Value::String(s) => s.value().to_string(),
        other => other.to_string().trim().trim_matches('"').to_string(),
    })
}

fn yaml_scalar(value: &serde_yaml_ng::Value) -> Option<String> {
    use serde_yaml_ng::Value as V;
    match value {
        V::String(s) => Some(s.clone()),
        V::Number(n) => Some(n.to_string()),
        V::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn json_scalar(value: &serde_json::Value) -> Option<String> {
    use serde_json::Value as V;
    match value {
        V::String(s) => Some(s.clone()),
        V::Number(n) => Some(n.to_string()),
        V::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn control_from_yaml(value: &serde_yaml_ng::Value) -> Control {
    use serde_yaml_ng::Value as V;
    let V::Mapping(map) = value else {
        return Control::default();
    };
    let get = |k: &str| map.get(V::String(k.to_string()));
    Control {
        extends: get("extends")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        members: get("members")
            .and_then(|v| v.as_sequence())
            .map(|s| s.iter().filter_map(yaml_scalar).collect())
            .unwrap_or_default(),
        matchers: match get("match") {
            Some(V::Mapping(m)) => m
                .iter()
                .filter_map(|(k, v)| Some((k.as_str()?.to_string(), yaml_scalar(v)?)))
                .collect(),
            _ => BTreeMap::new(),
        },
    }
}

fn control_from_json(value: &serde_json::Value) -> Control {
    let Some(map) = value.as_object() else {
        return Control::default();
    };
    Control {
        extends: map
            .get("extends")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        members: map
            .get("members")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(json_scalar).collect())
            .unwrap_or_default(),
        matchers: map
            .get("match")
            .and_then(|v| v.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), json_scalar(v)?)))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// ```xml
/// <answer-meta extends="base">
///   <member>98:fa:9b:50:d8:10</member>
///   <match serial="7ABC*"/>
/// </answer-meta>
/// ```
fn control_from_xml(root: &xml::Element) -> Control {
    let Some(meta) = root.elements().find(|e| e.name == XML_CONTROL_ELEMENT) else {
        return Control::default();
    };
    let mut control = Control {
        extends: meta
            .attrs
            .iter()
            .find(|(k, _)| k == "extends")
            .map(|(_, v)| v.trim().to_string())
            .filter(|s| !s.is_empty()),
        ..Default::default()
    };
    for child in meta.elements() {
        match child.name.as_str() {
            "member" => {
                let text = child.text();
                if !text.is_empty() {
                    control.members.push(text);
                }
            }
            "match" => {
                for (key, value) in &child.attrs {
                    control.matchers.insert(key.clone(), value.clone());
                }
            }
            _ => {}
        }
    }
    control
}

/// `# answer: extends rack-a`, `# answer: member 98:fa:…`, `# answer: match serial=7ABC*`
fn directive_of(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let body = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix("//"))?
        .trim_start();
    body.strip_prefix(TEXT_DIRECTIVE).map(str::trim)
}

fn control_from_text(text: &str) -> Control {
    let mut control = Control::default();
    for line in text.lines() {
        let Some(directive) = directive_of(line) else {
            continue;
        };
        let (verb, rest) = directive
            .split_once(char::is_whitespace)
            .unwrap_or((directive, ""));
        let rest = rest.trim();
        match verb {
            "extends" if !rest.is_empty() => control.extends = Some(rest.to_string()),
            "member" | "members" => {
                for value in rest.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    control.members.push(value.to_string());
                }
            }
            "match" => {
                for pair in rest.split_whitespace() {
                    if let Some((key, value)) = pair.split_once('=')
                        && !key.is_empty()
                    {
                        control.matchers.insert(key.to_string(), value.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    control
}

/// Replace `{{ key }}` in a piece of text.
///
/// Substitution happens on **parsed string values**, never on raw document text, so the
/// format's own serializer does the escaping — a value containing a quote cannot break
/// the TOML it lands in, and one containing `<` cannot break the XML. Line-oriented
/// formats have no structure to protect, so control characters are refused outright
/// rather than being allowed to inject a directive.
pub fn expand(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    if !text.contains("{{") {
        return Ok(text.to_string());
    }

    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err("unclosed `{{` in template".to_string());
        };

        let key = after[..end].trim();
        if key.is_empty() {
            return Err("empty `{{ }}` in template".to_string());
        }
        if !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!("{key:?} is not a usable template key"));
        }

        // A missing fact is an error, never an empty string: silently serving a
        // half-substituted answer is how a machine gets installed with the hostname
        // `node-.example.com`.
        let Some(value) = lookup(key) else {
            return Err(format!(
                "template needs {{{{ {key} }}}}, but this request carries no {key:?}"
            ));
        };
        if value.chars().any(|c| c.is_control()) {
            return Err(format!(
                "value for {key:?} contains a control character and will not be substituted"
            ));
        }
        out.push_str(&value);
        rest = &after[end + 2..];
    }

    out.push_str(rest);
    Ok(out)
}

impl Doc {
    /// Expand every `{{ key }}` in this document's string values.
    pub fn substitute(&mut self, lookup: &dyn Fn(&str) -> Option<String>) -> Result<(), String> {
        match &mut self.inner {
            Inner::Toml(doc) => substitute_toml(doc.as_item_mut(), lookup),
            Inner::Yaml(value) => substitute_yaml(value, lookup),
            Inner::Json(value) => substitute_json(value, lookup),
            Inner::Xml(doc) => substitute_xml(&mut doc.root, lookup),
            // No structure to protect here, so the whole body is one string.
            Inner::Text(text) => {
                *text = expand(text, lookup)?;
                Ok(())
            }
        }
    }

    /// Does this document ask for anything to be substituted?
    pub fn has_placeholders(&self) -> bool {
        self.render().contains("{{")
    }
}

fn substitute_toml(
    item: &mut toml_edit::Item,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    if let Some(table) = item.as_table_like_mut() {
        let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys {
            if let Some(child) = table.get_mut(&key) {
                substitute_toml(child, lookup)?;
            }
        }
        return Ok(());
    }
    match item.as_value_mut() {
        Some(toml_edit::Value::String(s)) => {
            let expanded = expand(s.value(), lookup)?;
            *item = toml_edit::value(expanded);
            Ok(())
        }
        Some(toml_edit::Value::Array(array)) => {
            for element in array.iter_mut() {
                if let toml_edit::Value::String(s) = element {
                    let expanded = expand(s.value(), lookup)?;
                    *element = toml_edit::Value::from(expanded);
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn substitute_yaml(
    value: &mut serde_yaml_ng::Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    use serde_yaml_ng::Value as V;
    match value {
        V::String(s) => *s = expand(s, lookup)?,
        V::Sequence(items) => {
            for item in items {
                substitute_yaml(item, lookup)?;
            }
        }
        V::Mapping(map) => {
            for (_, child) in map.iter_mut() {
                substitute_yaml(child, lookup)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn substitute_json(
    value: &mut serde_json::Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    use serde_json::Value as V;
    match value {
        V::String(s) => *s = expand(s, lookup)?,
        V::Array(items) => {
            for item in items {
                substitute_json(item, lookup)?;
            }
        }
        V::Object(map) => {
            for (_, child) in map.iter_mut() {
                substitute_json(child, lookup)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn substitute_xml(
    element: &mut xml::Element,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(), String> {
    for (_, value) in element.attrs.iter_mut() {
        *value = expand(value, lookup)?;
    }
    for node in element.children.iter_mut() {
        match node {
            xml::Node::Text(text) => *text = expand(text, lookup)?,
            xml::Node::Element(child) => substitute_xml(child, lookup)?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(kind: Kind, text: &str) -> Doc {
        Doc::parse(kind, text, "test").expect("fixture should parse")
    }

    fn layered(kind: Kind, base: &str, over: &str) -> String {
        let mut b = doc(kind, base);
        b.merge(&doc(kind, over)).expect("same kind");
        b.strip_control();
        b.render()
    }

    // ---- extensions --------------------------------------------------------

    #[test]
    fn extensions_map_to_the_right_format() {
        for (ext, kind) in [
            ("toml", Kind::Toml),
            ("TOML", Kind::Toml),
            ("yaml", Kind::Yaml),
            ("yml", Kind::Yaml),
            ("json", Kind::Json),
            ("ign", Kind::Json),
            ("xml", Kind::Xml),
            // XML under a name that says which XML it is, so a store holding both a
            // SUSE profile and a Windows unattend can keep them apart.
            ("autoyast", Kind::Xml),
            ("unattend", Kind::Xml),
            ("ks", Kind::Text),
            ("preseed", Kind::Text),
            ("seed", Kind::Text),
            ("cfg", Kind::Text),
            ("ipxe", Kind::Text),
        ] {
            assert_eq!(Kind::for_extension(ext), Some(kind), "{ext}");
        }
    }

    #[test]
    fn every_extension_the_allowlist_names_is_reachable_from_some_endpoint() {
        // An extension nothing can ask for is a document that can be stored and never
        // served — the worst kind of silence.
        for ext in [
            "toml", "yaml", "yml", "json", "ign", "xml", "autoyast", "unattend", "ks", "preseed",
            "seed", "cfg", "ipxe",
        ] {
            assert!(Kind::for_extension(ext).is_some(), "{ext} is not servable");
            let reachable = ENDPOINT_SEGMENTS
                .iter()
                .filter_map(|segment| endpoint_formats(segment))
                .any(|formats| formats.contains(&ext));
            assert!(reachable, "no endpoint ever asks for .{ext}");
        }
    }

    /// A canonical stem must never be an endpoint alias for a *different* format.
    ///
    /// The stem means nothing to this server — but somebody reading `ubuntu.yaml` in a
    /// directory will read it as "this machine as Ubuntu", and a table that wrote
    /// `debian.yaml` would be lying to them in the one place they look. Either the name
    /// is an alias that resolves to this very extension, or it is not an alias at all.
    #[test]
    fn a_canonical_stem_never_names_another_format() {
        for ext in [
            "toml", "yaml", "yml", "json", "ign", "xml", "autoyast", "unattend", "ks", "preseed",
            "seed", "cfg", "ipxe",
        ] {
            let stem = canonical_stem(ext);
            assert!(!stem.is_empty(), "{ext} has no name to be written under");
            if let Some(formats) = endpoint_formats(stem) {
                assert!(
                    formats.contains(&ext),
                    ".{ext} would be written as {stem}.{ext}, and {stem:?} is the endpoint \
                     for {formats:?}"
                );
            }
        }
    }

    /// Every alias the table names, with a format its installer would actually expect.
    ///
    /// The whole table rather than a sample: these are URLs baked into ISOs, so a
    /// segment that quietly stops resolving is a rollout that quietly stops working,
    /// and nobody finds out until the media is already in someone's hand.
    const ALIASES: [(&str, &str); 31] = [
        ("proxmox", "toml"),
        ("pve", "toml"),
        ("toml", "toml"),
        ("debian", "preseed"),
        ("preseed", "preseed"),
        ("rhel", "ks"),
        ("centos", "ks"),
        ("fedora", "ks"),
        ("alma", "ks"),
        ("rocky", "ks"),
        ("kickstart", "ks"),
        ("ks", "ks"),
        ("ubuntu", "yaml"),
        ("autoinstall", "yaml"),
        ("cloudinit", "yaml"),
        ("nocloud", "yaml"),
        ("yaml", "yaml"),
        ("yml", "yml"),
        ("suse", "autoyast"),
        ("opensuse", "autoyast"),
        ("autoyast", "autoyast"),
        ("windows", "unattend"),
        ("unattend", "unattend"),
        ("flatcar", "ign"),
        ("coreos", "ign"),
        ("ignition", "ign"),
        ("ign", "ign"),
        ("json", "json"),
        ("xml", "xml"),
        ("cfg", "cfg"),
        ("ipxe", "ipxe"),
    ];

    const ENDPOINT_SEGMENTS: [&str; 31] = {
        let mut names = [""; 31];
        let mut i = 0;
        while i < ALIASES.len() {
            names[i] = ALIASES[i].0;
            i += 1;
        }
        names
    };

    #[test]
    fn an_endpoint_asks_for_the_format_its_installer_expects() {
        for (segment, wanted) in ALIASES {
            let formats = endpoint_formats(segment)
                .unwrap_or_else(|| panic!("{segment} should name an endpoint"));
            assert!(formats.contains(&wanted), "{segment}: {formats:?}");
            // Case is not the caller's problem: an ISO may well carry /RHEL/ks.
            assert_eq!(
                endpoint_formats(&segment.to_ascii_uppercase()),
                Some(formats),
                "{segment} should match whatever case it arrives in"
            );
        }
    }

    #[test]
    fn an_endpoint_never_asks_for_a_format_meant_for_another_installer() {
        // The whole point: a kickstart client must not be handed TOML.
        let ks = endpoint_formats("rhel").unwrap();
        assert!(!ks.contains(&"toml") && !ks.contains(&"preseed"), "{ks:?}");
        let preseed = endpoint_formats("debian").unwrap();
        assert!(!preseed.contains(&"ks"), "{preseed:?}");
        // Even though both are opaque text to the merge engine.
        assert_eq!(Kind::for_extension("ks"), Kind::for_extension("preseed"));
    }

    #[test]
    fn suse_and_windows_can_be_told_apart_when_it_matters() {
        let suse = endpoint_formats("suse").unwrap();
        let windows = endpoint_formats("windows").unwrap();
        assert!(suse.contains(&"autoyast") && !suse.contains(&"unattend"));
        assert!(windows.contains(&"unattend") && !windows.contains(&"autoyast"));
        // Both parse as XML all the same.
        assert_eq!(Kind::for_extension("autoyast"), Some(Kind::Xml));
        assert_eq!(Kind::for_extension("unattend"), Some(Kind::Xml));
    }

    #[test]
    fn a_segment_naming_no_endpoint_constrains_nothing() {
        // `/answer` is what a Proxmox ISO is usually baked with; it must keep working.
        for segment in [
            "answer",
            "user-data",
            "config",
            "",
            "98fa9b50d810",
            "seed",
            "autoinstall-data",
        ] {
            assert!(endpoint_formats(segment).is_none(), "{segment:?}");
        }
    }

    #[test]
    fn unknown_extensions_are_not_picked_up() {
        // A stray README next to the answers must not become a candidate.
        for ext in ["md", "bak", "swp", "png", "txt", ""] {
            assert_eq!(Kind::for_extension(ext), None, "{ext}");
        }
    }

    // ---- structural merging, identical across formats -----------------------

    #[test]
    fn every_structured_format_overrides_a_scalar_and_keeps_its_siblings() {
        let cases = [
            (
                Kind::Toml,
                "[g]\na = \"keep\"\nb = \"old\"\n",
                "[g]\nb = \"new\"\n",
            ),
            (Kind::Yaml, "g:\n  a: keep\n  b: old\n", "g:\n  b: new\n"),
            (
                Kind::Json,
                r#"{"g":{"a":"keep","b":"old"}}"#,
                r#"{"g":{"b":"new"}}"#,
            ),
            (
                Kind::Xml,
                "<r><g><a>keep</a><b>old</b></g></r>",
                "<r><g><b>new</b></g></r>",
            ),
        ];
        for (kind, base, over) in cases {
            let out = layered(kind, base, over);
            assert!(
                out.contains("keep"),
                "{}: sibling lost\n{out}",
                kind.label()
            );
            assert!(
                out.contains("new"),
                "{}: not overridden\n{out}",
                kind.label()
            );
            assert!(!out.contains("old"), "{}: stale value\n{out}", kind.label());
        }
    }

    #[test]
    fn every_structured_format_replaces_a_list_rather_than_appending() {
        // Appending would make a list impossible to shorten from a higher layer.
        let cases = [
            (
                Kind::Toml,
                "disks = [\"sda\", \"sdb\"]\n",
                "disks = [\"nvme0n1\"]\n",
            ),
            (
                Kind::Yaml,
                "disks:\n  - sda\n  - sdb\n",
                "disks:\n  - nvme0n1\n",
            ),
            (
                Kind::Json,
                r#"{"disks":["sda","sdb"]}"#,
                r#"{"disks":["nvme0n1"]}"#,
            ),
            (
                Kind::Xml,
                "<r><disks><d>sda</d><d>sdb</d></disks></r>",
                "<r><disks><d>nvme0n1</d></disks></r>",
            ),
        ];
        for (kind, base, over) in cases {
            let out = layered(kind, base, over);
            assert!(out.contains("nvme0n1"), "{}: {out}", kind.label());
            assert!(
                !out.contains("sdb"),
                "{}: list appended\n{out}",
                kind.label()
            );
        }
    }

    #[test]
    fn every_structured_format_merges_nested_maps() {
        let cases = [
            (Kind::Toml, "[a.b]\nx = 1\ny = 2\n", "[a.b]\ny = 9\n"),
            (
                Kind::Yaml,
                "a:\n  b:\n    x: 1\n    y: 2\n",
                "a:\n  b:\n    y: 9\n",
            ),
            (
                Kind::Json,
                r#"{"a":{"b":{"x":1,"y":2}}}"#,
                r#"{"a":{"b":{"y":9}}}"#,
            ),
            (
                Kind::Xml,
                "<r><a><b><x>1</x><y>2</y></b></a></r>",
                "<r><a><b><y>9</y></b></a></r>",
            ),
        ];
        for (kind, base, over) in cases {
            let out = layered(kind, base, over);
            assert!(out.contains('9'), "{}: not overridden\n{out}", kind.label());
            assert!(
                out.contains('1'),
                "{}: deep sibling lost\n{out}",
                kind.label()
            );
        }
    }

    #[test]
    fn what_comes_out_can_always_be_read_back() {
        // Whatever is rendered goes straight to an installer.
        let cases = [
            (Kind::Toml, "[g]\na = 1\n", "[g]\nb = 2\n"),
            (Kind::Yaml, "g:\n  a: 1\n", "g:\n  b: 2\n"),
            (Kind::Json, r#"{"g":{"a":1}}"#, r#"{"g":{"b":2}}"#),
            (Kind::Xml, "<r><g a=\"1\"/></r>", "<r><g b=\"2\"/></r>"),
        ];
        for (kind, base, over) in cases {
            let out = layered(kind, base, over);
            Doc::parse(kind, &out, "rendered").unwrap_or_else(|e| {
                panic!(
                    "{}: merge produced unreadable output: {e}\n{out}",
                    kind.label()
                )
            });
        }
    }

    #[test]
    fn formats_cannot_be_mixed_in_one_answer() {
        let mut toml = doc(Kind::Toml, "a = 1\n");
        let err = toml
            .merge(&doc(Kind::Yaml, "b: 2\n"))
            .expect_err("must refuse");
        assert!(err.contains("same format"), "{err}");
    }

    // ---- control keys ------------------------------------------------------

    #[test]
    fn every_format_can_carry_its_control_keys() {
        let cases = [
            (
                Kind::Toml,
                "extends = \"base\"\nmembers = [\"aa:bb\", \"cc:dd\"]\n[match]\nserial = \"7ABC*\"\n[g]\nx = 1\n",
            ),
            (
                Kind::Yaml,
                "extends: base\nmembers: [aa:bb, cc:dd]\nmatch:\n  serial: 7ABC*\ng:\n  x: 1\n",
            ),
            (
                Kind::Json,
                r#"{"extends":"base","members":["aa:bb","cc:dd"],"match":{"serial":"7ABC*"},"g":{"x":1}}"#,
            ),
            (
                Kind::Xml,
                r#"<r><answer-meta extends="base"><member>aa:bb</member><member>cc:dd</member><match serial="7ABC*"/></answer-meta><g x="1"/></r>"#,
            ),
            (
                Kind::Text,
                "# answer: extends base\n# answer: member aa:bb, cc:dd\n# answer: match serial=7ABC*\nlang en_US\n",
            ),
        ];
        for (kind, text) in cases {
            let control = doc(kind, text).control();
            assert_eq!(control.extends.as_deref(), Some("base"), "{}", kind.label());
            assert_eq!(
                control.members.len(),
                2,
                "{}: {:?}",
                kind.label(),
                control.members
            );
            assert_eq!(
                control.matchers.get("serial").map(String::as_str),
                Some("7ABC*"),
                "{}",
                kind.label()
            );
        }
    }

    #[test]
    fn control_keys_never_reach_the_installer() {
        let cases = [
            (
                Kind::Toml,
                "extends = \"base\"\nmembers = [\"aa\"]\n[g]\nx = 1\n",
            ),
            (Kind::Yaml, "extends: base\nmembers: [aa]\ng:\n  x: 1\n"),
            (
                Kind::Json,
                r#"{"extends":"base","members":["aa"],"g":{"x":1}}"#,
            ),
            (
                Kind::Xml,
                r#"<r><answer-meta extends="base"><member>aa</member></answer-meta><g x="1"/></r>"#,
            ),
            (Kind::Text, "# answer: extends base\nlang en_US\n"),
        ];
        for (kind, text) in cases {
            let mut d = doc(kind, text);
            d.strip_control();
            let out = d.render();
            assert!(!out.contains("extends"), "{}: leaked\n{out}", kind.label());
            assert!(!out.contains("members"), "{}: leaked\n{out}", kind.label());
            assert!(
                !out.contains("answer-meta"),
                "{}: leaked\n{out}",
                kind.label()
            );
            // …while the real content stays.
            assert!(
                out.contains('x') || out.contains("lang"),
                "{}: content lost\n{out}",
                kind.label()
            );
        }
    }

    #[test]
    fn a_document_with_no_control_keys_says_so() {
        for (kind, text) in [
            (Kind::Toml, "[g]\nx = 1\n"),
            (Kind::Yaml, "g:\n  x: 1\n"),
            (Kind::Json, r#"{"g":{"x":1}}"#),
            (Kind::Xml, "<r><g x=\"1\"/></r>"),
            (Kind::Text, "lang en_US\n# an ordinary comment\n"),
        ] {
            assert!(doc(kind, text).control().is_empty(), "{}", kind.label());
        }
    }

    // ---- opaque text -------------------------------------------------------

    #[test]
    fn text_layers_by_concatenation_in_order() {
        // Not a merge, and the module says so: this is what kickstart and preseed get.
        let out = layered(
            Kind::Text,
            "# answer: extends base\nlang en_US\nkeyboard us\n",
            "keyboard fr\n",
        );
        assert!(out.contains("lang en_US"), "{out}");
        // Both directives survive, in order — the target format decides what that means.
        let first = out.find("keyboard us").expect("base kept");
        let second = out.find("keyboard fr").expect("overlay kept");
        assert!(first < second, "layer order must be preserved:\n{out}");
    }

    #[test]
    fn directives_are_recognised_after_either_comment_marker() {
        for line in [
            "# answer: extends base",
            "  //answer: extends base",
            "#answer:extends base",
        ] {
            let text = format!("{line}\nlang en_US\n");
            assert_eq!(
                doc(Kind::Text, &text).control().extends.as_deref(),
                Some("base"),
                "{line:?}"
            );
        }
    }

    #[test]
    fn an_ordinary_comment_is_left_alone() {
        let text = "# this is just a comment\n# answer: extends base\nlang en_US\n";
        let mut d = doc(Kind::Text, text);
        d.strip_control();
        let out = d.render();
        assert!(out.contains("just a comment"), "{out}");
        assert!(!out.contains("extends"), "{out}");
    }

    // ---- templating --------------------------------------------------------

    fn vars(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    fn substituted(
        kind: Kind,
        text: &str,
        pairs: &'static [(&'static str, &'static str)],
    ) -> String {
        let mut d = doc(kind, text);
        d.substitute(&vars(pairs)).expect("should substitute");
        d.render()
    }

    #[test]
    fn a_placeholder_is_replaced_in_every_format() {
        let cases = [
            (
                Kind::Toml,
                "[global]\nfqdn = \"node-{{ serial }}.example.com\"\n",
            ),
            (
                Kind::Yaml,
                "global:\n  fqdn: node-{{ serial }}.example.com\n",
            ),
            (
                Kind::Json,
                r#"{"global":{"fqdn":"node-{{ serial }}.example.com"}}"#,
            ),
            (
                Kind::Xml,
                "<r><fqdn>node-{{ serial }}.example.com</fqdn></r>",
            ),
            (
                Kind::Text,
                "d-i netcfg/get_hostname string node-{{ serial }}\n",
            ),
        ];
        for (kind, text) in cases {
            let out = substituted(kind, text, &[("serial", "7ABC123")]);
            assert!(out.contains("node-7ABC123"), "{}: {out}", kind.label());
            assert!(!out.contains("{{"), "{}: {out}", kind.label());
        }
    }

    #[test]
    fn a_value_cannot_break_the_document_it_lands_in() {
        // Substitution happens on parsed values, so the serializer escapes them. A
        // hostile serial must not be able to inject syntax.
        for (kind, text) in [
            (Kind::Toml, "[g]\nx = \"{{ serial }}\"\n"),
            (Kind::Yaml, "g:\n  x: \"{{ serial }}\"\n"),
            (Kind::Json, r#"{"g":{"x":"{{ serial }}"}}"#),
            (Kind::Xml, "<r><x>{{ serial }}</x></r>"),
        ] {
            let mut d = doc(kind, text);
            d.substitute(&|_: &str| Some(String::from("a\"b'c<d>e&f")))
                .expect("should substitute");
            let out = d.render();
            Doc::parse(kind, &out, "substituted").unwrap_or_else(|e| {
                panic!("{}: injection broke the document: {e}\n{out}", kind.label())
            });
        }
    }

    #[test]
    fn a_placeholder_works_inside_an_array_and_an_attribute() {
        let out = substituted(
            Kind::Toml,
            "[disk-setup]\ndisk-list = [\"{{ disk }}\", \"sdb\"]\n",
            &[("disk", "nvme0n1")],
        );
        assert!(out.contains("nvme0n1") && out.contains("sdb"), "{out}");

        let out = substituted(
            Kind::Xml,
            "<r><n name=\"{{ serial }}\"/></r>",
            &[("serial", "7ABC123")],
        );
        assert!(out.contains("name=\"7ABC123\""), "{out}");
    }

    #[test]
    fn a_missing_fact_is_an_error_not_an_empty_string() {
        // Silently serving `node-.example.com` is how a machine gets installed wrong.
        let mut d = doc(Kind::Toml, "[g]\nfqdn = \"node-{{ serial }}\"\n");
        let err = d
            .substitute(&vars(&[("mac", "aabb")]))
            .expect_err("must refuse");
        assert!(err.contains("serial"), "{err}");
    }

    #[test]
    fn a_malformed_placeholder_is_refused() {
        for text in [
            "[g]\nx = \"{{ unclosed\"\n",
            "[g]\nx = \"{{ }}\"\n",
            "[g]\nx = \"{{ has space }}\"\n",
        ] {
            let mut d = doc(Kind::Toml, text);
            assert!(d.substitute(&vars(&[("x", "y")])).is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_control_character_is_never_substituted() {
        // In a line-oriented format a newline would inject a directive.
        let mut d = doc(Kind::Text, "hostname {{ name }}\n");
        let err = d
            .substitute(&|_: &str| Some(String::from("evil\nrootpw hunter2")))
            .expect_err("must refuse");
        assert!(err.contains("control character"), "{err}");
    }

    #[test]
    fn a_document_without_placeholders_is_untouched() {
        for (kind, text) in [
            (Kind::Toml, "[g]\nx = \"plain\"\n"),
            (Kind::Text, "lang en_US\n"),
        ] {
            let before = doc(kind, text).render();
            let after = substituted(kind, text, &[]);
            assert_eq!(before, after, "{}", kind.label());
        }
    }

    #[test]
    fn placeholders_are_detected_before_they_are_needed() {
        assert!(doc(Kind::Toml, "[g]\nx = \"{{ a }}\"\n").has_placeholders());
        assert!(!doc(Kind::Toml, "[g]\nx = \"plain\"\n").has_placeholders());
    }

    // ---- broken input ------------------------------------------------------

    #[test]
    fn malformed_documents_are_refused_with_the_origin_named() {
        for (kind, bad) in [
            (Kind::Toml, "this = = not toml"),
            (Kind::Yaml, "a:\n  - b\n c: bad indent"),
            (Kind::Json, "{\"a\":}"),
            (Kind::Xml, "<a></b>"),
        ] {
            let err = Doc::parse(kind, bad, "answers/98fa9b.x").expect_err(kind.label());
            assert!(err.contains("answers/98fa9b.x"), "{err}");
        }
        // Text has no syntax to be wrong about.
        assert!(Doc::parse(Kind::Text, "anything at all", "x").is_ok());
    }
}
