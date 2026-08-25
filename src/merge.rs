//! Deep-merging answer files, so a group can carry what a rack shares and a machine
//! file only carries what differs.
//!
//! Merge rules, chosen to be predictable rather than clever:
//!   * tables merge recursively (including inline and dotted tables);
//!   * any other value is replaced outright by the higher layer;
//!   * arrays replace, they do not concatenate — appending would make it impossible to
//!     shorten a list, and silent accumulation across layers is hard to reason about.
//!
//! Layers apply lowest to highest: the group chain first, the machine file last, so the
//! machine always wins.

use toml_edit::{DocumentMut, Item};

/// Keys this server understands that Proxmox does not. They steer resolution and must
/// never reach the installer.
pub const CONTROL_KEYS: [&str; 2] = ["extends", "members"];

/// Merge `over` onto `base`, in place.
pub fn merge_into(base: &mut DocumentMut, over: &DocumentMut) {
    merge_item(base.as_item_mut(), over.as_item());
}

fn merge_item(base: &mut Item, over: &Item) {
    // `as_table_like` covers both `[table]` and `{ inline = "table" }`, so a group can
    // use one style and a machine the other without surprises.
    match (base.as_table_like_mut(), over.as_table_like()) {
        (Some(base_table), Some(over_table)) => {
            for (key, over_value) in over_table.iter() {
                match base_table.get_mut(key) {
                    Some(base_value) => merge_item(base_value, over_value),
                    None => {
                        base_table.insert(key, over_value.clone());
                    }
                }
            }
        }
        // Not both tables: the higher layer wins wholesale.
        _ => *base = over.clone(),
    }
}

/// Strip the keys that are ours, not Proxmox's, from the top level of a document.
pub fn strip_control_keys(doc: &mut DocumentMut) {
    let table = doc.as_table_mut();
    for key in CONTROL_KEYS {
        table.remove(key);
    }
}

/// Read `extends` from a document, if present.
pub fn extends_of(doc: &DocumentMut) -> Option<String> {
    doc.as_table()
        .get("extends")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read `members` from a document as a list of strings.
pub fn members_of(doc: &DocumentMut) -> Vec<String> {
    let Some(item) = doc.as_table().get("members") else {
        return Vec::new();
    };
    let Some(array) = item.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|v| v.as_str())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(s: &str) -> DocumentMut {
        s.parse().expect("valid toml fixture")
    }

    fn merged(base: &str, over: &str) -> String {
        let mut b = doc(base);
        merge_into(&mut b, &doc(over));
        strip_control_keys(&mut b);
        b.to_string()
    }

    #[test]
    fn disjoint_tables_are_combined() {
        let out = merged(
            "[global]\ncountry = \"fr\"\n",
            "[disk-setup]\nfilesystem = \"zfs\"\n",
        );
        assert!(out.contains("country = \"fr\""), "{out}");
        assert!(out.contains("filesystem = \"zfs\""), "{out}");
    }

    #[test]
    fn the_higher_layer_overrides_a_scalar() {
        let out = merged(
            "[global]\nkeyboard = \"us\"\ncountry = \"fr\"\n",
            "[global]\nkeyboard = \"fr\"\n",
        );
        assert!(out.contains("keyboard = \"fr\""), "{out}");
        assert!(!out.contains("\"us\""), "{out}");
        // The key it did not mention must survive.
        assert!(out.contains("country = \"fr\""), "{out}");
    }

    #[test]
    fn nested_tables_merge_rather_than_replace() {
        let out = merged(
            "[disk-setup]\nfilesystem = \"zfs\"\n\n[disk-setup.zfs]\nraid = \"raid1\"\nashift = 12\n",
            "[disk-setup.zfs]\nraid = \"raid10\"\n",
        );
        assert!(out.contains("raid = \"raid10\""), "{out}");
        // Sibling keys deeper in the tree must not be lost.
        assert!(out.contains("ashift = 12"), "{out}");
        assert!(out.contains("filesystem = \"zfs\""), "{out}");
    }

    #[test]
    fn dotted_and_inline_tables_merge_like_ordinary_ones() {
        let out = merged(
            "[disk-setup]\nzfs = { raid = \"raid1\", ashift = 12 }\n",
            "[disk-setup]\nzfs.raid = \"raid10\"\n",
        );
        assert!(out.contains("raid10"), "{out}");
        assert!(out.contains("ashift"), "{out}");
        assert!(!out.contains("raid1\""), "{out}");
    }

    #[test]
    fn arrays_replace_and_do_not_accumulate() {
        // Appending would make a list impossible to shorten from a higher layer.
        let out = merged(
            "[disk-setup]\ndisk_list = [\"sda\", \"sdb\", \"sdc\"]\n",
            "[disk-setup]\ndisk_list = [\"nvme0n1\"]\n",
        );
        assert!(out.contains("nvme0n1"), "{out}");
        assert!(!out.contains("sdb"), "{out}");
    }

    #[test]
    fn a_scalar_can_become_a_table_and_the_reverse() {
        // Assert on the parsed result, not the spacing: replacing a table with a scalar
        // leaves the key's original decor, so the text may read `value= 3`. That is
        // valid TOML, and the contract is the value, not the whitespace.
        let out = merged("value = 3\n", "[value]\nnested = true\n");
        let parsed = doc(&out);
        assert_eq!(
            parsed["value"]["nested"].as_bool(),
            Some(true),
            "a scalar must be replaceable by a table: {out}"
        );

        let out = merged("[value]\nnested = true\n", "value = 3\n");
        let parsed = doc(&out);
        assert_eq!(
            parsed["value"].as_integer(),
            Some(3),
            "a table must be replaceable by a scalar: {out}"
        );
    }

    #[test]
    fn merged_output_is_always_reparseable() {
        // Whatever the merge produces is fed straight to the installer, so it has to be
        // valid TOML — including in the awkward shape-changing cases above.
        for (base, over) in [
            (
                "[global]\nkeyboard = \"us\"\n",
                "[global]\nkeyboard = \"fr\"\n",
            ),
            ("value = 3\n", "[value]\nnested = true\n"),
            ("[value]\nnested = true\n", "value = 3\n"),
            ("[a.b.c]\nx = 1\n", "[a]\nb = { c = { y = 2 } }\n"),
            ("x = [1, 2, 3]\n", "x = []\n"),
        ] {
            let out = merged(base, over);
            out.parse::<DocumentMut>()
                .unwrap_or_else(|e| panic!("merge produced invalid toml: {e}\n{out}"));
        }
    }

    #[test]
    fn control_keys_never_reach_the_output() {
        let out = merged(
            "members = [\"98:fa:9b:50:d8:10\"]\n[global]\ncountry = \"fr\"\n",
            "extends = \"rack-a\"\n[global]\nkeyboard = \"fr\"\n",
        );
        assert!(!out.contains("extends"), "{out}");
        assert!(!out.contains("members"), "{out}");
        assert!(out.contains("country"), "{out}");
    }

    #[test]
    fn comments_survive_the_merge() {
        // toml_edit preserves formatting; a served file should stay readable, and the
        // comments an admin wrote in a group are worth keeping.
        let out = merged(
            "# the whole rack runs ZFS\n[disk-setup]\nfilesystem = \"zfs\"\n",
            "[global]\ncountry = \"fr\"\n",
        );
        assert!(out.contains("# the whole rack runs ZFS"), "{out}");
    }

    #[test]
    fn extends_and_members_are_read_correctly() {
        let d = doc("extends = \"base\"\nmembers = [\"aa:bb\", \"cc:dd\"]\n");
        assert_eq!(extends_of(&d).as_deref(), Some("base"));
        assert_eq!(members_of(&d), vec!["aa:bb", "cc:dd"]);

        let empty = doc("[global]\nx = 1\n");
        assert_eq!(extends_of(&empty), None);
        assert!(members_of(&empty).is_empty());
    }

    #[test]
    fn a_malformed_extends_is_ignored_rather_than_trusted() {
        assert_eq!(extends_of(&doc("extends = 42\n")), None);
        assert_eq!(extends_of(&doc("extends = \"  \"\n")), None);
        assert!(members_of(&doc("members = \"not-a-list\"\n")).is_empty());
    }
}
