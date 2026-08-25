//! The same behaviour, proved against every store.
//!
//! Two backends is how two backends drift apart. This file exists so that they cannot:
//! every case runs twice, once over a directory of TOML files and once over SQLite, and
//! asserts the identical outcome. A behaviour that only holds for one of them fails
//! here.
//!
//! Anything store-specific (atomic renames, the schema) is tested at the bottom.

use rescriptum::facts::Facts;
use rescriptum::select::{Answers, Resolution};
use rescriptum::store::{FileStore, SqliteStore, Store, StoreWrite};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn scratch(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("pve-stores-{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("scratch dir");
    p
}

/// A store under test, plus the scratch directory to clean up afterwards.
struct Subject {
    label: &'static str,
    store: Arc<dyn StoreWrite>,
    dir: PathBuf,
}

impl Drop for Subject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Every store, ready to be written to.
fn subjects() -> Vec<Subject> {
    let file_dir = scratch("files");
    let db_dir = scratch("sqlite");
    vec![
        Subject {
            label: "files",
            store: Arc::new(FileStore::new(&file_dir)),
            dir: file_dir,
        },
        Subject {
            label: "sqlite",
            store: Arc::new(
                SqliteStore::open(db_dir.join("answers.db")).expect("open sqlite store"),
            ),
            dir: db_dir,
        },
    ]
}

/// Run one behavioural case against every store.
fn for_each_store(case: impl Fn(&'static str, &Arc<dyn StoreWrite>)) {
    for subject in subjects() {
        case(subject.label, &subject.store);
    }
}

fn answers(store: &Arc<dyn StoreWrite>) -> Answers {
    // Upcast the write handle to the read trait the resolver takes.
    Answers::new(store.clone() as Arc<dyn Store>)
}

/// Shaped after what the Proxmox installer actually posts, so selector tests have
/// something real to select on.
fn body(mac: &str) -> String {
    format!(
        r#"{{"product":{{"short":"pve"}},
            "dmi":{{"system":{{"manufacturer":"Dell Inc.","product":"PowerEdge R620",
                              "serial":"7ABC123","uuid":"4c4c4544-0037-4110-8043-b7c04f313233"}}}},
            "network_interfaces":[{{"name":"eno1","mac":"{mac}"}}],
            "disks":[{{"path":"/dev/sda"}}]}}"#
    )
}

fn facts_for(mac: &str) -> Facts {
    Facts::new(None, body(mac).as_bytes())
}

fn resolve(store: &Arc<dyn StoreWrite>, mac: &str) -> Option<Resolution> {
    answers(store)
        .resolve(&facts_for(mac))
        .expect("resolve should not fail")
}

// ---------------------------------------------------------------------------
// Behaviour that must hold identically for every store
// ---------------------------------------------------------------------------

#[test]
fn a_machine_document_is_served() {
    for_each_store(|label, store| {
        store
            .put_machine("98-fa-9b-50-d8-10", "toml", "[global]\nkeyboard = \"fr\"\n")
            .expect("put machine");
        let r = resolve(store, "98:fa:9b:50:d8:10").unwrap_or_else(|| panic!("{label}: no answer"));
        assert_eq!(r.machine.as_deref(), Some("98-fa-9b-50-d8-10"), "{label}");
        assert!(r.body.contains("\"fr\""), "{label}: {}", r.body);
    });
}

#[test]
fn separator_style_never_matters() {
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "[global]\nx = 1\n")
            .unwrap();
        for written in ["98:fa:9b:50:d8:10", "98-FA-9B-50-D8-10", "98fa9b50d810"] {
            assert!(
                resolve(store, written).is_some(),
                "{label}: {written} should match"
            );
        }
    });
}

#[test]
fn the_default_is_the_fallback_and_only_that() {
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"specific\"\n")
            .unwrap();
        store
            .put_default("toml", "marker = \"fallback\"\n")
            .unwrap();

        let hit = resolve(store, "98:fa:9b:50:d8:10").unwrap();
        assert!(!hit.used_default, "{label}: a match must beat the default");

        let miss = resolve(store, "11:22:33:44:55:66").unwrap();
        assert!(miss.used_default, "{label}");
        assert!(miss.body.contains("fallback"), "{label}");
    });
}

#[test]
fn nothing_matching_and_no_default_is_nothing() {
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "x = 1\n")
            .unwrap();
        assert!(
            resolve(store, "11:22:33:44:55:66").is_none(),
            "{label}: should be a 404"
        );
    });
}

#[test]
fn a_group_answers_for_its_members() {
    for_each_store(|label, store| {
        store
            .put_group(
                "rack-a",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\"]\n[global]\ncountry = \"fr\"\n",
            )
            .unwrap();
        let r = resolve(store, "98:fa:9b:50:d8:10").unwrap_or_else(|| panic!("{label}: no answer"));
        assert_eq!(r.group.as_deref(), Some("rack-a"), "{label}");
        assert!(r.body.contains("country"), "{label}");
        assert!(!r.body.contains("members"), "{label}: control key leaked");
    });
}

#[test]
fn a_machine_layers_on_top_of_its_group() {
    for_each_store(|label, store| {
        store
            .put_group(
                "rack-a",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\"]\n\
                 [global]\ncountry = \"fr\"\nkeyboard = \"fr\"\n",
            )
            .unwrap();
        store
            .put_machine("98-fa-9b-50-d8-10", "toml", "[global]\nkeyboard = \"us\"\n")
            .unwrap();

        let r = resolve(store, "98:fa:9b:50:d8:10").unwrap();
        assert_eq!(r.machine.as_deref(), Some("98-fa-9b-50-d8-10"), "{label}");
        assert_eq!(r.group.as_deref(), Some("rack-a"), "{label}");
        assert!(r.body.contains("\"us\""), "{label}: machine must win");
        assert!(r.body.contains("country"), "{label}: group must survive");
    });
}

#[test]
fn a_group_can_extend_another_group() {
    for_each_store(|label, store| {
        store
            .put_group(
                "base",
                "toml",
                "[global]\ncountry = \"fr\"\ntimezone = \"UTC\"\n",
            )
            .unwrap();
        store
            .put_group(
                "rack-a",
                "toml",
                "extends = \"base\"\nmembers = [\"98:fa:9b:50:d8:10\"]\n\
                 [global]\ntimezone = \"Europe/Paris\"\n",
            )
            .unwrap();

        let r = resolve(store, "98:fa:9b:50:d8:10").unwrap();
        assert!(r.body.contains("country"), "{label}: inherited from base");
        assert!(r.body.contains("Europe/Paris"), "{label}: child wins");
        assert!(!r.body.contains("extends"), "{label}: control key leaked");
    });
}

#[test]
fn extending_an_unknown_group_is_an_error_not_a_half_built_answer() {
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "extends = \"ghost\"\n")
            .unwrap();
        assert!(
            answers(store)
                .resolve(&facts_for("98:fa:9b:50:d8:10"))
                .is_err(),
            "{label}: must refuse to serve"
        );
    });
}

#[test]
fn an_unparseable_document_is_an_error_not_a_wrong_answer() {
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "this is = = not toml\n")
            .unwrap();
        assert!(
            answers(store)
                .resolve(&facts_for("98:fa:9b:50:d8:10"))
                .is_err(),
            "{label}: must refuse to serve"
        );
    });
}

#[test]
fn a_broken_group_does_not_stop_the_healthy_ones() {
    for_each_store(|label, store| {
        store.put_group("broken", "toml", "not = = toml\n").unwrap();
        store
            .put_group(
                "rack-a",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\"]\n[global]\nx = 1\n",
            )
            .unwrap();

        let a = answers(store);
        assert!(
            !a.problems().unwrap().is_empty(),
            "{label}: should report it"
        );
        let r = a.resolve(&facts_for("98:fa:9b:50:d8:10")).unwrap();
        assert_eq!(
            r.unwrap().group.as_deref(),
            Some("rack-a"),
            "{label}: the healthy rack still installs"
        );
    });
}

#[test]
fn a_group_cycle_is_reported_and_dropped() {
    for_each_store(|label, store| {
        store.put_group("a", "toml", "extends = \"b\"\n").unwrap();
        store.put_group("b", "toml", "extends = \"a\"\n").unwrap();
        let a = answers(store);
        assert!(
            a.problems().unwrap().iter().any(|p| p.contains("cycle")),
            "{label}: {:?}",
            a.problems().unwrap()
        );
        assert!(a.group_names().unwrap().is_empty(), "{label}");
    });
}

#[test]
fn writes_are_visible_to_a_live_resolver() {
    // The admin API changes things while installs are in flight; a resolver that has
    // already cached must notice.
    for_each_store(|label, store| {
        let a = answers(store);
        assert!(
            a.resolve(&facts_for("98:fa:9b:50:d8:10"))
                .unwrap()
                .is_none(),
            "{label}"
        );

        store
            .put_machine("98fa9b50d810", "toml", "marker = \"added\"\n")
            .unwrap();
        let r = a.resolve(&facts_for("98:fa:9b:50:d8:10")).unwrap();
        assert!(r.is_some(), "{label}: an added machine must be picked up");

        assert!(
            store.delete_machine("98fa9b50d810", "toml").unwrap(),
            "{label}"
        );
        assert!(
            a.resolve(&facts_for("98:fa:9b:50:d8:10"))
                .unwrap()
                .is_none(),
            "{label}: a removed machine must stop being served"
        );
    });
}

#[test]
fn deleting_something_absent_reports_it_rather_than_failing() {
    for_each_store(|label, store| {
        assert!(!store.delete_machine("nope", "toml").unwrap(), "{label}");
        assert!(!store.delete_group("nope", "toml").unwrap(), "{label}");
        assert!(!store.delete_default("toml").unwrap(), "{label}");
    });
}

#[test]
fn a_put_replaces_rather_than_duplicates() {
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"first\"\n")
            .unwrap();
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"second\"\n")
            .unwrap();

        let r = resolve(store, "98:fa:9b:50:d8:10").unwrap();
        assert!(r.body.contains("second"), "{label}: {}", r.body);
        assert!(!r.body.contains("first"), "{label}: {}", r.body);
        assert_eq!(answers(store).machine_ids().unwrap().len(), 1, "{label}");
    });
}

#[test]
fn matching_is_deterministic_whatever_the_store_order() {
    for_each_store(|label, store| {
        store
            .put_machine("aabbccddeeff", "toml", "marker = \"second\"\n")
            .unwrap();
        store
            .put_machine("001122334455", "toml", "marker = \"first\"\n")
            .unwrap();
        let both = Facts::new(
            None,
            br#"{"macs":["00:11:22:33:44:55","aa:bb:cc:dd:ee:ff"]}"#,
        );

        for _ in 0..5 {
            let r = answers(store).resolve(&both).unwrap().unwrap();
            assert_eq!(r.machine.as_deref(), Some("001122334455"), "{label}");
        }
    });
}

// ---------------------------------------------------------------------------
// Store-specific
// ---------------------------------------------------------------------------

#[test]
fn the_file_store_writes_atomically_and_leaves_no_scratch_files() {
    let dir = scratch("atomic");
    let store = FileStore::new(&dir);
    store
        .put_machine("98fa9b50d810", "toml", "marker = \"x\"\n")
        .unwrap();
    store
        .put_group("rack-a", "toml", "[global]\nx = 1\n")
        .unwrap();

    // A reader must never meet a half-written answer, so nothing temporary survives.
    let stray: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.contains("tmp"))
        .collect();
    assert!(stray.is_empty(), "left temporary files behind: {stray:?}");
    assert!(dir.join("98fa9b50d810.toml").is_file());
    assert!(dir.join("groups/rack-a.toml").is_file());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_sqlite_store_survives_being_reopened() {
    let dir = scratch("reopen");
    let db = dir.join("answers.db");
    {
        let store = SqliteStore::open(&db).unwrap();
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"persisted\"\n")
            .unwrap();
        store
            .put_group("rack-a", "toml", "members = [\"aa:bb\"]\n[g]\nx = 1\n")
            .unwrap();
    }
    let store: Arc<dyn StoreWrite> = Arc::new(SqliteStore::open(&db).unwrap());
    let r = resolve(&store, "98:fa:9b:50:d8:10").expect("data should have persisted");
    assert!(r.body.contains("persisted"));
    assert_eq!(answers(&store).group_names().unwrap().len(), 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn the_sqlite_store_refuses_a_database_from_a_newer_version() {
    // Opening a database written by a newer binary and guessing at its schema is how
    // you corrupt a fleet's configuration.
    let dir = scratch("newer");
    let db = dir.join("answers.db");
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA user_version = 9999").unwrap();
    }
    let err = match SqliteStore::open(&db) {
        Err(e) => e,
        Ok(_) => panic!("must refuse a database from a newer version"),
    };
    assert!(err.to_string().contains("newer"), "{err}");
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Formats — again, identically on both stores
// ---------------------------------------------------------------------------

#[test]
fn every_extension_the_allowlist_names_can_be_served_end_to_end() {
    // The allowlist has thirteen extensions; the tables above exercise six of them, and
    // an extension nobody ever stores is an extension nobody notices breaking. One
    // machine per format so no two candidates can claim the same request.
    const CASES: [(&str, &str, &str, &str); 13] = [
        ("toml", "/proxmox/answer", "marker = \"m-toml\"\n", "m-toml"),
        ("yaml", "/ubuntu/user-data", "marker: m-yaml\n", "m-yaml"),
        ("yml", "/ubuntu/user-data", "marker: m-yml\n", "m-yml"),
        ("json", "/json/config", "{\"marker\":\"m-json\"}", "m-json"),
        ("ign", "/flatcar/config", "{\"marker\":\"m-ign\"}", "m-ign"),
        (
            "xml",
            "/xml/profile",
            "<r><marker>m-xml</marker></r>",
            "m-xml",
        ),
        (
            "autoyast",
            "/suse/profile",
            "<r><marker>m-autoyast</marker></r>",
            "m-autoyast",
        ),
        (
            "unattend",
            "/windows/unattend",
            "<unattend><marker>m-unattend</marker></unattend>",
            "m-unattend",
        ),
        ("ks", "/rhel/ks", "# kickstart\nlang m-ks\n", "m-ks"),
        (
            "preseed",
            "/debian/preseed",
            "d-i marker string m-preseed\n",
            "m-preseed",
        ),
        (
            "seed",
            "/debian/preseed",
            "d-i marker string m-seed\n",
            "m-seed",
        ),
        ("cfg", "/cfg/node", "marker=m-cfg\n", "m-cfg"),
        (
            "ipxe",
            "/ipxe/boot",
            "#!ipxe\nset marker m-ipxe\n",
            "m-ipxe",
        ),
    ];

    for_each_store(|label, store| {
        for (i, (format, endpoint, document, marker)) in CASES.iter().enumerate() {
            let mac = format!("aa:bb:cc:00:00:{i:02x}");
            let id = mac.replace(':', "");
            store
                .put_machine(&id, format, document)
                .unwrap_or_else(|e| panic!("{label}/{format}: {e}"));

            let r = at(store, endpoint, &mac)
                .unwrap_or_else(|| panic!("{label}/{format}: nothing answered at {endpoint}"));
            assert!(r.body.contains(marker), "{label}/{format}: {}", r.body);
            // The endpoint must have served *this* format, not merely something.
            assert_eq!(&r.format_name, format, "{label}: at {endpoint}");
        }
    });
}

#[test]
fn every_supported_format_round_trips_through_a_store() {
    for_each_store(|label, store| {
        for (format, endpoint, body, marker) in [
            ("toml", "/proxmox/answer", "marker = \"toml\"\n", "toml"),
            ("yaml", "/ubuntu/user-data", "marker: yaml\n", "yaml"),
            ("ign", "/flatcar/config", "{\"marker\":\"json\"}", "json"),
            (
                "autoyast",
                "/suse/profile",
                "<r><marker>xml</marker></r>",
                "xml",
            ),
            ("ks", "/rhel/ks", "# kickstart\nlang fr_FR\n", "fr_FR"),
            (
                "preseed",
                "/debian/preseed",
                "d-i marker string deb\n",
                "deb",
            ),
        ] {
            store.put_machine("98fa9b50d810", format, body).unwrap();
            let r = at(store, endpoint, "98:fa:9b:50:d8:10")
                .unwrap_or_else(|| panic!("{label}/{format}: no answer at {endpoint}"));
            assert!(r.body.contains(marker), "{label}/{format}: {}", r.body);
        }
    });
}

#[test]
fn a_second_format_is_a_second_answer_not_a_replacement() {
    // The earlier model treated one machine as having one answer, so writing another
    // format replaced it. That was wrong: a machine's answer is specific to the OS it
    // is for, and the same hardware legitimately has one of each.
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"proxmox\"\n")
            .unwrap();
        store
            .put_machine("98fa9b50d810", "yaml", "marker: ubuntu\n")
            .unwrap();

        let ids = answers(store).machine_ids().unwrap();
        assert_eq!(ids.len(), 2, "{label}: both must survive: {ids:?}");

        let t = at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").expect("toml");
        assert!(t.body.contains("proxmox"), "{label}: {}", t.body);
        let y = at(store, "/ubuntu/user-data", "98:fa:9b:50:d8:10").expect("yaml");
        assert!(y.body.contains("ubuntu"), "{label}: {}", y.body);

        // Rewriting one format leaves the other alone, and does replace itself.
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"proxmox-v2\"\n")
            .unwrap();
        assert_eq!(answers(store).machine_ids().unwrap().len(), 2, "{label}");
        let t = at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").unwrap();
        assert!(
            t.body.contains("proxmox-v2") && !t.body.contains("\"proxmox\""),
            "{}",
            t.body
        );

        // And deleting one names the format it means.
        assert!(
            store.delete_machine("98fa9b50d810", "toml").unwrap(),
            "{label}"
        );
        assert!(
            at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").is_none(),
            "{label}"
        );
        assert!(
            at(store, "/ubuntu/user-data", "98:fa:9b:50:d8:10").is_some(),
            "{label}"
        );
    });
}

#[test]
fn a_format_nothing_can_serve_is_refused() {
    for_each_store(|label, store| {
        assert!(
            store.put_machine("98fa9b50d810", "md", "# notes").is_err(),
            "{label}"
        );
        assert!(
            store.put_group("rack-a", "exe", "binary").is_err(),
            "{label}"
        );
    });
}

#[test]
fn layers_of_different_formats_are_refused_rather_than_half_served() {
    for_each_store(|label, store| {
        store
            .put_group(
                "rack-a",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\"]\n[g]\nx = 1\n",
            )
            .unwrap();
        store
            .put_machine("98fa9b50d810", "yaml", "g:\n  y: 2\n")
            .unwrap();

        let err = answers(store)
            .resolve(&facts_for("98:fa:9b:50:d8:10"))
            .expect_err("a YAML machine cannot be laid over a TOML group");
        assert!(err.to_string().contains("same format"), "{label}: {err}");
    });
}

#[test]
fn a_selector_claims_a_machine_the_filename_does_not() {
    for_each_store(|label, store| {
        store
            .put_group(
                "dell",
                "toml",
                "[match]\nproduct = \"PowerEdge R620\"\n[global]\nkeyboard = \"fr\"\n",
            )
            .unwrap();
        // The body carries `"product": "PowerEdge R620"` but a MAC nobody named.
        let r = resolve(store, "11:22:33:44:55:66")
            .unwrap_or_else(|| panic!("{label}: the selector should have claimed it"));
        assert_eq!(r.group.as_deref(), Some("dell"), "{label}");
    });
}

#[test]
fn naming_a_machine_outranks_any_selector() {
    for_each_store(|label, store| {
        store
            .put_group(
                "broad",
                "toml",
                "[match]\nproduct = \"PowerEdge R620\"\n[g]\npick = \"selector\"\n",
            )
            .unwrap();
        store
            .put_group(
                "named",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\"]\n[g]\npick = \"named\"\n",
            )
            .unwrap();

        let r = resolve(store, "98:fa:9b:50:d8:10").unwrap();
        assert_eq!(r.group.as_deref(), Some("named"), "{label}: {:?}", r.group);
    });
}

#[test]
fn among_selectors_the_more_specific_one_wins() {
    for_each_store(|label, store| {
        store
            .put_group(
                "a-broad",
                "toml",
                "[match]\nmanufacturer = \"Dell Inc.\"\n[g]\npick = \"broad\"\n",
            )
            .unwrap();
        store
            .put_group(
                "z-narrow",
                "toml",
                "[match]\nmanufacturer = \"Dell Inc.\"\nproduct = \"PowerEdge R620\"\n[g]\npick = \"narrow\"\n",
            )
            .unwrap();

        // Sorted order would pick `a-broad`; specificity must win instead.
        let r = resolve(store, "11:22:33:44:55:66").unwrap();
        assert_eq!(
            r.group.as_deref(),
            Some("z-narrow"),
            "{label}: {:?}",
            r.group
        );
    });
}

// ---------------------------------------------------------------------------
// SQLite migrations
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The endpoint asks for a format; only documents of that format may answer
// ---------------------------------------------------------------------------

fn at(store: &Arc<dyn StoreWrite>, path: &str, mac: &str) -> Option<Resolution> {
    answers(store)
        .resolve(&Facts::from_request(Some(path), None, body(mac).as_bytes()))
        .expect("resolve should not fail")
}

#[test]
fn one_machine_can_be_answered_as_two_operating_systems() {
    // A machine's answer is specific to the OS it is for, so the same hardware holds
    // one document as Proxmox and another as Debian — two answers to two questions.
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"as-proxmox\"\n")
            .unwrap();
        store
            .put_machine("98fa9b50d810", "preseed", "d-i marker string as-debian\n")
            .unwrap();

        let p = at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").expect("proxmox");
        assert!(p.body.contains("as-proxmox"), "{label}: {}", p.body);

        let d = at(store, "/debian/preseed", "98:fa:9b:50:d8:10").expect("debian");
        assert!(d.body.contains("as-debian"), "{label}: {}", d.body);

        // Both are really stored; neither replaced the other.
        assert_eq!(answers(store).machine_ids().unwrap().len(), 2, "{label}");
    });
}

#[test]
fn an_endpoint_never_serves_another_installers_format() {
    // A kickstart client handed TOML is the failure this exists to prevent.
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"proxmox\"\n")
            .unwrap();

        assert!(
            at(store, "/rhel/ks", "98:fa:9b:50:d8:10").is_none(),
            "{label}: a TOML answer must not reach a kickstart endpoint"
        );
        assert!(
            at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").is_some(),
            "{label}"
        );
    });
}

#[test]
fn kickstart_and_preseed_are_told_apart_though_both_are_opaque_text() {
    // They share a Kind, so the filter has to work on the extension, not the family.
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "ks", "lang fr_FR\n")
            .unwrap();
        store
            .put_machine("98fa9b50d810", "preseed", "d-i locale string fr_FR\n")
            .unwrap();

        let ks = at(store, "/rhel/ks", "98:fa:9b:50:d8:10").expect("kickstart");
        assert!(ks.body.contains("lang fr_FR"), "{label}: {}", ks.body);

        let pre = at(store, "/debian/preseed", "98:fa:9b:50:d8:10").expect("preseed");
        assert!(pre.body.contains("d-i locale"), "{label}: {}", pre.body);
    });
}

#[test]
fn a_url_naming_no_endpoint_is_unconstrained() {
    // `/answer` is what a Proxmox ISO is usually baked with, and must keep working.
    for_each_store(|label, store| {
        store
            .put_machine("98fa9b50d810", "toml", "marker = \"any\"\n")
            .unwrap();
        assert!(
            at(store, "/answer", "98:fa:9b:50:d8:10").is_some(),
            "{label}"
        );
        assert!(at(store, "/", "98:fa:9b:50:d8:10").is_some(), "{label}");
    });
}

#[test]
fn the_default_is_chosen_by_format_too() {
    for_each_store(|label, store| {
        store
            .put_default("toml", "marker = \"toml-default\"\n")
            .unwrap();
        store.put_default("ks", "lang en_US\n").unwrap();

        let t = at(store, "/proxmox/answer", "00:00:00:00:00:01").expect("toml default");
        assert!(t.body.contains("toml-default"), "{label}: {}", t.body);

        let k = at(store, "/rhel/ks", "00:00:00:00:00:01").expect("ks default");
        assert!(k.body.contains("lang en_US"), "{label}: {}", k.body);

        // A format with no default of its own gets nothing rather than the wrong one.
        assert!(
            at(store, "/ubuntu/user-data", "00:00:00:00:00:01").is_none(),
            "{label}"
        );
    });
}

// ---------------------------------------------------------------------------
// Grouping keeps working, per format
// ---------------------------------------------------------------------------

#[test]
fn grouping_still_works_within_each_format() {
    for_each_store(|label, store| {
        store
            .put_group(
                "rack",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\"]\n[global]\ncountry = \"fr\"\n",
            )
            .unwrap();
        store
            .put_machine("98fa9b50d810", "toml", "[global]\nkeyboard = \"us\"\n")
            .unwrap();

        let r = at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").expect("group + machine");
        assert_eq!(r.group.as_deref(), Some("rack"), "{label}");
        assert!(
            r.body.contains("country"),
            "{label}: group survives: {}",
            r.body
        );
        assert!(
            r.body.contains("\"us\""),
            "{label}: machine wins: {}",
            r.body
        );
    });
}

#[test]
fn a_group_of_one_format_never_claims_a_request_for_another() {
    for_each_store(|label, store| {
        store
            .put_group(
                "rack",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\"]\n[g]\nx = 1\n",
            )
            .unwrap();
        store
            .put_group(
                "rack",
                "ks",
                "# answer: member 98:fa:9b:50:d8:10\nlang fr_FR\n",
            )
            .unwrap();

        let t = at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").expect("toml group");
        assert!(t.body.contains("x = 1"), "{label}: {}", t.body);

        let k = at(store, "/rhel/ks", "98:fa:9b:50:d8:10").expect("ks group");
        assert!(k.body.contains("lang fr_FR"), "{label}: {}", k.body);
    });
}

#[test]
fn inheritance_stays_inside_one_format() {
    for_each_store(|label, store| {
        store
            .put_group("base", "toml", "[global]\ncountry = \"fr\"\n")
            .unwrap();
        store
            .put_group("base", "ks", "timezone Europe/Paris\n")
            .unwrap();
        store
            .put_group(
                "rack",
                "toml",
                "extends = \"base\"\nmembers = [\"98:fa:9b:50:d8:10\"]\n[global]\nkeyboard = \"fr\"\n",
            )
            .unwrap();

        let r = at(store, "/proxmox/answer", "98:fa:9b:50:d8:10").expect("should resolve");
        assert!(
            r.body.contains("country"),
            "{label}: inherited the TOML base: {}",
            r.body
        );
        assert!(
            !r.body.contains("Europe/Paris"),
            "{label}: took the kickstart base: {}",
            r.body
        );
    });
}

#[test]
fn a_change_made_by_another_process_is_picked_up_without_a_restart() {
    // `version()` on the SQLite store is an in-process atomic, because it is called on
    // every request and a query there would defeat the cache. The cost of that choice is
    // that a write from *another* process — the admin API of a second instance, a
    // hand-edited database — does not move it. The one-second reload backstop is what
    // covers it, and this is the only thing proving that it does.
    let dir = scratch("cross-process");
    let db = dir.join("answers.db");

    let serving: Arc<dyn StoreWrite> = Arc::new(SqliteStore::open(&db).expect("open"));
    let resolver = answers(&serving);
    assert!(
        resolver
            .resolve(&facts_for("98:fa:9b:50:d8:10"))
            .expect("resolve")
            .is_none(),
        "nothing is stored yet"
    );

    // A second handle on the same file, standing in for another process.
    let elsewhere: Arc<dyn StoreWrite> = Arc::new(SqliteStore::open(&db).expect("reopen"));
    elsewhere
        .put_machine("98fa9b50d810", "toml", "marker = \"from-elsewhere\"\n")
        .expect("write");

    // Poll rather than sleep exactly once: the backstop is a second, and a loaded machine
    // should not turn a timing guarantee into a flaky test.
    let mut seen = None;
    for _ in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Some(r) = resolver
            .resolve(&facts_for("98:fa:9b:50:d8:10"))
            .expect("resolve")
        {
            seen = Some(r);
            break;
        }
    }
    let seen = seen.expect("the backstop should have re-read the store");
    assert!(seen.body.contains("from-elsewhere"), "{}", seen.body);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_machine_document_can_be_claimed_by_a_selector_too() {
    // Documented and never asserted: "a machine document may carry a `match` block too —
    // useful for whatever machine is currently in this chassis slot". Its filename says
    // nothing about the hardware, so only the selector can claim it.
    for_each_store(|label, store| {
        store
            .put_machine(
                "chassis-slot-3",
                "toml",
                "[match]\nserial = \"7ABC*\"\n\n[global]\nmarker = \"by-slot\"\n",
            )
            .unwrap();

        let r = resolve(store, "aa:bb:cc:dd:ee:ff")
            .unwrap_or_else(|| panic!("{label}: the selector should have claimed it"));
        assert_eq!(r.machine.as_deref(), Some("chassis-slot-3"), "{label}");
        assert!(r.body.contains("by-slot"), "{label}: {}", r.body);
    });
}

#[test]
fn a_group_member_without_a_document_of_its_own_has_no_machine_variable() {
    // A known limitation, pinned so that changing it is deliberate rather than accidental.
    // `machine` is the identifier of the machine *document* that matched, so a machine
    // claimed only by a group's `members` has none. The guide tells you to use a request
    // fact such as {{ mac }} there instead.
    for_each_store(|label, store| {
        store
            .put_group(
                "rack",
                "toml",
                "members = [\"98:fa:9b:50:d8:10\", \"98:fa:9b:50:d8:11\"]\n\
                 [global]\nfqdn = \"node-{{ machine }}\"\n",
            )
            .unwrap();

        // No document for this one, so nothing can fill the placeholder.
        let bare = answers(store).resolve(&facts_for("98:fa:9b:50:d8:11"));
        let message = bare
            .expect_err(&format!("{label}: should refuse"))
            .to_string();
        assert!(message.contains("{{ machine }}"), "{label}: {message}");

        // Give it a document and the same group renders.
        store
            .put_machine("98fa9b50d810", "toml", "[global]\nkeyboard = \"fr\"\n")
            .unwrap();
        let r = resolve(store, "98:fa:9b:50:d8:10")
            .unwrap_or_else(|| panic!("{label}: should resolve"));
        assert!(r.body.contains("node-98fa9b50d810"), "{label}: {}", r.body);
    });
}

#[test]
fn an_answers_directory_that_appears_later_is_served_on_the_next_request() {
    // Not after the backstop expires: the directory's arrival changes the version from
    // "unreadable" to an mtime, so the cache is rebuilt at once. Someone who created the
    // directory in response to the startup warning should not have to wonder whether to
    // wait, or restart.
    let dir = scratch("appears-later");
    let answers_dir = dir.join("not-yet");

    // A directory that does not exist has no mtime, so `version()` is None.
    let store: Arc<dyn StoreWrite> = Arc::new(FileStore::new(&answers_dir));
    assert!(store.version().is_none(), "the premise of this test");
    let resolver = answers(&store);
    assert!(
        resolver
            .resolve(&facts_for("98:fa:9b:50:d8:10"))
            .unwrap()
            .is_none(),
        "nothing to serve yet"
    );

    // Well inside the one-second backstop, so the version change is what has to do it.
    fs::create_dir_all(&answers_dir).unwrap();
    fs::write(
        answers_dir.join("98fa9b50d810.toml"),
        "marker = \"appeared\"\n",
    )
    .unwrap();

    let r = resolver
        .resolve(&facts_for("98:fa:9b:50:d8:10"))
        .expect("resolve")
        .expect("the store must be re-read at once, not after the backstop");
    assert!(r.body.contains("appeared"), "{}", r.body);

    let _ = fs::remove_dir_all(&dir);
}
