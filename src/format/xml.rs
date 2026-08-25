//! A small XML tree, and rules for layering one over another.
//!
//! XML has no inherent notion of "the same element in both documents", so merging it
//! means choosing one. These rules are chosen to be **predictable rather than clever**,
//! and to line up with how the structured formats behave:
//!
//! * children are paired by element name, and by a discriminating attribute
//!   (`name`, `id`, `key`, `alias`) when one is present — that is what makes
//!   `<component name="Shell-Setup">` in an unattend.xml mergeable;
//! * a name occurring **once** on both sides merges recursively, like a table;
//! * a name occurring **more than once** is a list, and the overlay's whole set
//!   replaces the base's — the same rule as arrays everywhere else in this project,
//!   for the same reason: appending would make a list impossible to shorten;
//! * AutoYaST's `config:type="list"` marks a list explicitly, and is honoured;
//! * attributes merge key by key, the overlay winning;
//! * text content is replaced by the overlay when it has any.
//!
//! What this does **not** do is understand any particular schema. It cannot know that
//! two differently-named elements mean the same thing, or that an order matters. Render
//! the result and check it before trusting a rack to it.

use quick_xml::Reader;
use quick_xml::escape::escape;
use quick_xml::events::Event;

/// Attributes that identify *which* element this is among siblings of the same name.
/// `pass` is here for Windows: an unattend.xml carries several `<settings pass="…">`
/// blocks, and without it they would look like an anonymous list and be replaced
/// wholesale by whichever one an overlay happened to mention.
const DISCRIMINATORS: [&str; 5] = ["name", "id", "key", "alias", "pass"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Whether the source carried an `<?xml …?>` declaration. Reconstructed on render
    /// as a standard one rather than preserved verbatim — an installer cares that it is
    /// there and well-formed, not how it was spelled.
    pub declared: bool,
    /// The doctype, kept verbatim. AutoYaST profiles open with `<!DOCTYPE profile>` and
    /// dropping it silently would change what SUSE is handed.
    pub doctype: Option<String>,
    pub root: Element,
}

impl Element {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// How this element is identified among its siblings.
    fn key(&self) -> (String, Option<String>) {
        let discriminator = DISCRIMINATORS
            .iter()
            .find_map(|d| self.attr(d))
            .map(|v| v.to_string());
        (self.name.clone(), discriminator)
    }

    /// Explicitly declared a list by AutoYaST.
    fn declared_list(&self) -> bool {
        self.attr("config:type").is_some_and(|v| v == "list")
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// The element's own text, ignoring child elements.
    pub fn text(&self) -> String {
        self.children
            .iter()
            .filter_map(|n| match n {
                Node::Text(t) => Some(t.as_str()),
                Node::Element(_) => None,
            })
            .collect::<String>()
            .trim()
            .to_string()
    }

    pub fn remove_children_named(&mut self, name: &str) -> Vec<Element> {
        let mut removed = Vec::new();
        self.children.retain(|node| match node {
            Node::Element(e) if e.name == name => {
                removed.push(e.clone());
                false
            }
            _ => true,
        });
        removed
    }
}

pub fn parse(text: &str) -> Result<Document, String> {
    let mut reader = Reader::from_str(text);
    reader.config_mut().trim_text(false);

    let mut declared = false;
    let mut doctype: Option<String> = None;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;

    loop {
        match reader.read_event() {
            Err(e) => {
                return Err(format!(
                    "invalid XML at byte {}: {e}",
                    reader.buffer_position()
                ));
            }
            Ok(Event::Eof) => break,

            Ok(Event::Decl(_)) => declared = true,
            Ok(Event::DocType(d)) => {
                doctype = Some(String::from_utf8_lossy(d.as_ref()).trim().to_string());
            }

            Ok(Event::Start(start)) => {
                let element = element_from(&start)?;
                stack.push(element);
            }
            Ok(Event::Empty(start)) => {
                let element = element_from(&start)?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(element)),
                    None => root = Some(element),
                }
            }
            Ok(Event::End(_)) => {
                let Some(done) = stack.pop() else {
                    return Err("unbalanced closing tag".to_string());
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(done)),
                    None => root = Some(done),
                }
            }
            // Stray text outside the root is not ours to keep.
            Ok(Event::Text(_)) | Ok(Event::CData(_)) if stack.is_empty() => {}
            Ok(Event::Text(t)) => {
                // Kept verbatim, including whitespace: `Element::text` trims the joined
                // result, so indentation between child elements still comes out empty
                // while the spacing *inside* a value survives.
                let decoded = t.decode().map_err(|e| e.to_string())?;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Text(decoded.to_string()));
                }
            }
            // An entity reference is a separate event, not part of the text around it.
            // Dropping them silently welds the surrounding fragments together — which
            // is how `1 &lt; 2 &amp; 3` became `123`.
            Ok(Event::GeneralRef(r)) => {
                let name = String::from_utf8_lossy(r.as_ref()).to_string();
                let resolved = match name.as_str() {
                    "lt" => Some("<".to_string()),
                    "gt" => Some(">".to_string()),
                    "amp" => Some("&".to_string()),
                    "apos" => Some("'".to_string()),
                    "quot" => Some("\"".to_string()),
                    other => other
                        .strip_prefix('#')
                        .and_then(
                            |n| match n.strip_prefix('x').or_else(|| n.strip_prefix('X')) {
                                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                                None => n.parse::<u32>().ok(),
                            },
                        )
                        .and_then(char::from_u32)
                        .map(|c| c.to_string()),
                };
                match resolved {
                    Some(text) => {
                        if let Some(parent) = stack.last_mut() {
                            parent.children.push(Node::Text(text));
                        }
                    }
                    // An entity we cannot resolve would silently change the document.
                    None => return Err(format!("unsupported entity reference &{name};")),
                }
            }
            Ok(Event::CData(c)) => {
                let text = String::from_utf8_lossy(c.as_ref()).to_string();
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(Node::Text(text));
                }
            }
            Ok(_) => {}
        }
    }

    if !stack.is_empty() {
        return Err(format!("unclosed element <{}>", stack[0].name));
    }
    match root {
        Some(root) => Ok(Document {
            declared,
            doctype,
            root,
        }),
        None => Err("no root element".to_string()),
    }
}

fn element_from(start: &quick_xml::events::BytesStart<'_>) -> Result<Element, String> {
    let name = String::from_utf8_lossy(start.name().as_ref()).to_string();
    let mut attrs = Vec::new();
    for attr in start.attributes() {
        let attr = attr.map_err(|e| format!("bad attribute in <{name}>: {e}"))?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        // Unescape explicitly rather than via the attribute helper, whose signature
        // has churned across quick-xml releases.
        let raw = String::from_utf8_lossy(attr.value.as_ref());
        let value = quick_xml::escape::unescape(&raw)
            .map_err(|e| format!("bad attribute value in <{name}>: {e}"))?
            .to_string();
        attrs.push((key, value));
    }
    Ok(Element {
        name,
        attrs,
        children: Vec::new(),
    })
}

/// Layer `over` on top of `base`, in place.
pub fn merge(base: &mut Element, over: &Element) {
    // Attributes: overlay wins, key by key.
    for (key, value) in &over.attrs {
        match base.attrs.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value.clone(),
            None => base.attrs.push((key.clone(), value.clone())),
        }
    }

    // Text: the overlay replaces it, but only if it actually has some.
    let over_text = over.text();
    if !over_text.is_empty() {
        base.children.retain(|n| !matches!(n, Node::Text(_)));
        base.children.push(Node::Text(over_text));
    }

    for name in distinct_names(over) {
        let over_group: Vec<Element> = over
            .elements()
            .filter(|e| e.name == name)
            .cloned()
            .collect();
        let base_group: Vec<&Element> = base.elements().filter(|e| e.name == name).collect();

        // Siblings that carry a discriminating attribute are a keyed collection, not an
        // anonymous list: `<component name="Shell-Setup">` and `<component name="Other">`
        // are two different things to be merged individually, not a list to replace.
        let keyed = base_group.iter().any(|e| e.key().1.is_some())
            || over_group.iter().any(|e| e.key().1.is_some());
        let declared_list = base_group.iter().any(|e| e.declared_list())
            || over_group.iter().any(|e| e.declared_list());
        let repeated = over_group.len() > 1 || base_group.len() > 1;

        if declared_list || (repeated && !keyed) {
            // An anonymous repeated element is a list, and lists replace wholesale.
            base.remove_children_named(&name);
            for element in over_group {
                base.children.push(Node::Element(element));
            }
            continue;
        }

        for over_child in over_group {
            let paired = base
                .children
                .iter_mut()
                .filter_map(|n| match n {
                    Node::Element(e) if e.name == name => Some(e),
                    _ => None,
                })
                .find(|e| e.key() == over_child.key());

            match paired {
                Some(base_child) => merge(base_child, &over_child),
                None => base.children.push(Node::Element(over_child)),
            }
        }
    }
}

fn distinct_names(element: &Element) -> Vec<String> {
    let mut names = Vec::new();
    for child in element.elements() {
        if !names.contains(&child.name) {
            names.push(child.name.clone());
        }
    }
    names
}

pub fn render(document: &Document) -> String {
    let mut out = String::new();
    if document.declared {
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    }
    if let Some(doctype) = &document.doctype {
        out.push_str("<!DOCTYPE ");
        out.push_str(doctype);
        out.push_str(">\n");
    }
    render_element(&document.root, 0, &mut out);
    out
}

fn render_element(element: &Element, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    out.push_str(&pad);
    out.push('<');
    out.push_str(&element.name);
    for (key, value) in &element.attrs {
        out.push(' ');
        out.push_str(key);
        out.push_str("=\"");
        out.push_str(&escape(value.as_str()));
        out.push('"');
    }

    let text = element.text();
    let children: Vec<&Element> = element.elements().collect();

    if children.is_empty() && text.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push('>');

    if children.is_empty() {
        out.push_str(&escape(text.as_str()));
        out.push_str("</");
        out.push_str(&element.name);
        out.push_str(">\n");
        return;
    }

    out.push('\n');
    if !text.is_empty() {
        out.push_str(&"  ".repeat(depth + 1));
        out.push_str(&escape(text.as_str()));
        out.push('\n');
    }
    for child in children {
        render_element(child, depth + 1, out);
    }
    out.push_str(&pad);
    out.push_str("</");
    out.push_str(&element.name);
    out.push_str(">\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(xml: &str) -> String {
        render(&parse(xml).expect("valid xml"))
    }

    fn merged(base: &str, over: &str) -> String {
        let mut b = parse(base).expect("base");
        let o = parse(over).expect("over");
        merge(&mut b.root, &o.root);
        render(&b)
    }

    #[test]
    fn a_document_survives_a_round_trip() {
        let out = round("<profile><global><country>fr</country></global></profile>");
        assert!(out.contains("<country>fr</country>"), "{out}");
        assert!(out.contains("<profile>"), "{out}");
    }

    #[test]
    fn attributes_and_empty_elements_survive() {
        let out = round(r#"<a x="1"><b y="2"/></a>"#);
        assert!(out.contains(r#"x="1""#), "{out}");
        assert!(out.contains(r#"<b y="2"/>"#), "{out}");
    }

    #[test]
    fn special_characters_are_escaped_on_the_way_out() {
        let out = round("<a><b>1 &lt; 2 &amp; 3</b></a>");
        assert!(out.contains("&lt;"), "{out}");
        assert!(out.contains("&amp;"), "{out}");
        // And it must parse again — the real contract.
        parse(&out).expect("rendered output must reparse");
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        for bad in ["<a>", "<a></b>", "not xml", "", "<a attr=></a>"] {
            assert!(parse(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn a_single_element_on_both_sides_merges() {
        let out = merged(
            "<p><global><country>fr</country><keyboard>fr</keyboard></global></p>",
            "<p><global><keyboard>us</keyboard></global></p>",
        );
        assert!(out.contains("<country>fr</country>"), "kept: {out}");
        assert!(out.contains("<keyboard>us</keyboard>"), "overridden: {out}");
        assert!(!out.contains(">fr</keyboard>"), "{out}");
    }

    #[test]
    fn a_repeated_element_is_a_list_and_replaces_wholesale() {
        // Appending would make it impossible to shorten the list from a higher layer.
        let out = merged(
            "<p><users><u>a</u><u>b</u><u>c</u></users></p>",
            "<p><users><u>z</u></users></p>",
        );
        assert!(out.contains("<u>z</u>"), "{out}");
        assert!(!out.contains("<u>a</u>"), "{out}");
        assert!(!out.contains("<u>b</u>"), "{out}");
    }

    #[test]
    fn autoyast_list_typing_is_honoured() {
        let out = merged(
            r#"<p><partitions config:type="list"><part>sda</part></partitions></p>"#,
            r#"<p><partitions config:type="list"><part>nvme0n1</part></partitions></p>"#,
        );
        assert!(out.contains("nvme0n1"), "{out}");
        assert!(
            !out.contains(">sda<"),
            "a declared list must replace: {out}"
        );
    }

    #[test]
    fn siblings_are_paired_by_a_discriminating_attribute() {
        // This is what makes an unattend.xml <component name="..."> mergeable.
        let out = merged(
            r#"<u><component name="Shell-Setup"><TimeZone>UTC</TimeZone><Skip>1</Skip></component><component name="Other"><X>1</X></component></u>"#,
            r#"<u><component name="Shell-Setup"><TimeZone>Europe/Paris</TimeZone></component></u>"#,
        );
        assert!(
            out.contains("Europe/Paris"),
            "the named one is updated: {out}"
        );
        assert!(
            out.contains("<Skip>1</Skip>"),
            "its siblings survive: {out}"
        );
        assert!(
            out.contains(r#"name="Other""#),
            "the other one is untouched: {out}"
        );
    }

    #[test]
    fn a_keyed_sibling_only_the_overlay_has_is_added() {
        let out = merged(
            r#"<u><component name="A"><x>1</x></component></u>"#,
            r#"<u><component name="B"><y>2</y></component></u>"#,
        );
        assert!(out.contains(r#"name="A""#), "{out}");
        assert!(out.contains(r#"name="B""#), "{out}");
    }

    #[test]
    fn entity_references_survive_intact() {
        // `1 &lt; 2 &amp; 3` must not come back as `123`.
        let doc = parse("<a><b>1 &lt; 2 &amp; 3</b></a>").expect("valid");
        let b = doc.root.elements().next().unwrap();
        assert_eq!(b.text(), "1 < 2 & 3");
    }

    #[test]
    fn numeric_entities_are_resolved_and_unknown_ones_refused() {
        let doc = parse("<a>&#65;&#x42;</a>").expect("valid");
        assert_eq!(doc.root.text(), "AB");
        // Silently dropping an entity we do not know would change the document.
        assert!(parse("<a>&nbsp;</a>").is_err());
    }

    #[test]
    fn attributes_merge_key_by_key() {
        let out = merged(r#"<a x="1" y="2"/>"#, r#"<a y="9" z="3"/>"#);
        assert!(out.contains(r#"x="1""#), "{out}");
        assert!(out.contains(r#"y="9""#), "{out}");
        assert!(out.contains(r#"z="3""#), "{out}");
    }

    #[test]
    fn an_element_only_the_overlay_has_is_added() {
        let out = merged("<p><a>1</a></p>", "<p><b>2</b></p>");
        assert!(out.contains("<a>1</a>"), "{out}");
        assert!(out.contains("<b>2</b>"), "{out}");
    }

    #[test]
    fn empty_overlay_text_does_not_erase_the_base() {
        let out = merged("<p><a>keep</a></p>", "<p><a/></p>");
        assert!(out.contains("keep"), "{out}");
    }

    #[test]
    fn a_declaration_is_reproduced_when_the_source_had_one() {
        let out = round(r#"<?xml version="1.0" encoding="utf-8"?><a><b>1</b></a>"#);
        assert!(out.starts_with("<?xml"), "{out}");
        parse(&out).expect("must reparse");
        // And is not invented when there was none.
        assert!(!round("<a><b>1</b></a>").starts_with("<?xml"));
    }

    #[test]
    fn a_doctype_survives() {
        // AutoYaST profiles open with one, and losing it changes what SUSE is handed.
        let out = round("<?xml version=\"1.0\"?><!DOCTYPE profile><profile><x>1</x></profile>");
        assert!(out.contains("<!DOCTYPE profile>"), "{out}");
        parse(&out).expect("must reparse");
    }

    #[test]
    fn windows_settings_blocks_are_paired_by_their_pass() {
        // Several <settings pass="…"> in one unattend.xml are distinct things, not a
        // list to be replaced by whichever one the overlay mentions.
        let out = merged(
            r#"<unattend><settings pass="windowsPE"><a>1</a></settings><settings pass="specialize"><b>2</b></settings></unattend>"#,
            r#"<unattend><settings pass="specialize"><b>9</b></settings></unattend>"#,
        );
        assert!(
            out.contains(r#"pass="windowsPE""#),
            "the other pass survives: {out}"
        );
        assert!(out.contains("<b>9</b>"), "the named pass is updated: {out}");
        assert!(out.contains("<a>1</a>"), "{out}");
    }

    #[test]
    fn a_merged_document_always_reparses() {
        // Whatever comes out is fed to an installer, so it has to be valid.
        for (base, over) in [
            ("<p><a>1</a></p>", "<p><a>2</a></p>"),
            (r#"<p><a x="&amp;"/></p>"#, r#"<p><a y="&lt;"/></p>"#),
            ("<p><l><i>1</i><i>2</i></l></p>", "<p><l><i>3</i></l></p>"),
        ] {
            let out = merged(base, over);
            parse(&out).unwrap_or_else(|e| panic!("merge produced invalid XML: {e}\n{out}"));
        }
    }
}
