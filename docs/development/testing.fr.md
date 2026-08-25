---
title: Tests
description: Où un test a sa place, la suite de conformité des deux stores, et les assertions qui ont réellement attrapé quelque chose.
sidebar:
  label: Tests
  order: 8
---

# Tests

308 tests. `cargo test` les fait tous tourner en quelques secondes.

```bash
cargo test                                # tout
cargo test <nom>                          # un seul, par sous-chaîne
cargo test -- --nocapture                 # afficher stdout
cargo test --all-features                 # ce que lance la CI
```

## Où un test a sa place

| Suite | Cas | Pour |
|---|---|---|
| `tests/stores.rs` | 38 | **chaque comportement, contre les deux stores** |
| `tests/integration.rs` | 45 | le vrai binaire sur une vraie socket |
| `src/select.rs` | 27 | normalisation, scoring, superposition, remplissage de templates |
| `src/format/mod.rs` | 27 | parsing, fusion, clés de contrôle, alias d'endpoint |
| `src/facts.rs` | 22 | parsing de query, aplatissement JSON, globbing |
| `tests/admin.rs` | 26 | l'API d'administration de bout en bout, formats compris |
| `tests/cli.rs` | 29 | `render`, `check`, `import`, `export` et le fichier d'environnement — contre le vrai binaire |
| `src/log.rs` | 4 | lecture des niveaux, et l'arithmétique d'horodatage |
| `src/format/xml.rs` | 18 | l'arbre XML — appariement, entités, fidélité |
| `src/config.rs` | 19 | l'environnement, et ce qui refuse de démarrer |
| `src/merge.rs` | 11 | la fusion profonde TOML |
| `tests/guards.rs` | 7 | le jeton de réponse, et le verrouillage qui délibérément n'existe pas |
| `src/envfile.rs` | 14 | le parseur du fichier d'environnement, et ce qu'il refuse |
| `src/admin.rs`, `src/capture.rs`, `src/store/mod.rs` | 21 | comportement unitaire |

## `tests/stores.rs` — la suite de conformité

Chaque cas de comportement tourne **deux fois**, une par store, et affirme le résultat
identique. Cette suite est ce qui empêche deux backends de diverger.

**Un nouveau comportement a sa place là, pas dans un test propre à un store.** Un test qui
couvre un seul backend prouve la moitié de ce qu'il prétend — et la moitié qu'il ne couvre pas
est exactement là où se cache une divergence.

## `tests/cli.rs` — les commandes qu'on dit aux gens de lancer

`check` est ce que [`deploy.sh`](./building.md#déployer-un-build) lance avant d'expédier
quoi que ce soit, et ce que la documentation dit de mettre en CI — donc **son code de
sortie est un contrat**, pas une commodité. La séparation stdout/stderr de `render` en est
un autre : le document part sur stdout pour que `render … > answer.toml` donne un fichier
utilisable, et la ligne de provenance part sur stderr pour qu'elle ne s'y retrouve pas.

Également épinglé ici : l'aller-retour `import` → `export` est **identique octet pour
octet**, commentaires et mise en forme compris. C'est ce qui rend la base sûre à adopter
*et* sûre à quitter ; si cela cesse d'être exact, `export` n'est plus une porte de sortie.

## `tests/integration.rs` — contre le vrai binaire

Il démarre le binaire réel sur un **port éphémère** et lui parle en HTTP. Le binaire affiche
l'adresse qu'il a bindée, donc il n'y a ni course sur le port ni « on dort et on espère ».

Cette suite existe parce que certains échecs sont invisibles aux tests unitaires. L'exemple le
plus clair : hyper **panique à l'exécution** si `header_read_timeout` est défini sans
`.timer(…)`. Ça compile. Seule une vraie connexion le trouve.

Explicitement couverts :

- une **requête tronquée**, et une **sans `Content-Length`** ;
- un `Content-Length` aberrant — *et* un corps chunked qui dépasse le plafond en cours de
  route, l'autre chemin d'entrée, où la limite saute en pleine lecture ;
- une méthode inconnue, un corps vide, un corps de 1 Mo, un corps qui n'est pas de
  l'UTF-8 valide ;
- le plafond de connexions : au-delà, un `503` immédiat plutôt qu'une file — et le permis
  qui revient ensuite ;
- **et, après chacun de ces cas, que le serveur répond encore.** C'est cette dernière
  assertion qui compte — la maltraitance n'a d'intérêt que si le serveur y survit.

> **`cargo test` ne reconstruit pas `target/debug/rescriptum`.** Une vérification manuelle
> contre un binaire périmé a un jour « reproduit » un bug déjà corrigé. Reconstruisez avant de
> triturer le binaire à la main.

## Vérifier qu'un test peut échouer

Un test qui passe pour la mauvaise raison est pire que pas de test : il annonce une
couverture qui n'existe pas. Avant de faire confiance à un nouveau test, cassez ce qu'il
garde et regardez-le rougir.

Un test de cette suite n'a pas survécu à cette vérification. Il prétendait protéger la
clause `version.is_some()` du cache du listing ; en la retirant, il restait vert — parce
qu'avec l'un comme l'autre store, une version n'est illisible que lorsque le store est aussi
vide, si bien que la clause ne peut pas se déclencher. Le test prouve quelque chose de réel
— un répertoire qui apparaît après le démarrage est servi dès la requête suivante — et le
dit maintenant.

## Assertions à copier

- **Affirmez sur des valeurs parsées, pas sur la mise en forme.** Remplacer une table par un
  scalaire laisse la décoration d'origine de la clé, donc la sortie peut se lire `value= 3` —
  du TOML valide, un texte différent. Une comparaison de chaînes échoue là pour la mauvaise
  raison, ou passe pour une mauvaise raison.
- **Les tests d'invalidation de cache doivent partager une seule instance d'`Answers`.** Un
  test qui en construit une nouvelle à chaque appel contourne complètement le cache et ne
  prouve silencieusement rien.
- **`Config::from_lookup` prend une closure**, pour que les tests de configuration ne touchent
  jamais l'environnement du processus — et ne se courent donc jamais après sous un runner
  parallèle.
- **Vérifiez que l'ancien texte a bien été trouvé avant d'écrire.** Deux patchs
  `python`/`sed` dans l'histoire de ce projet n'ont silencieusement rien matché et n'ont été
  attrapés qu'en vérifiant le nombre de tests ensuite.

## Les exemples de réponses sont aussi un test

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

[`examples/`](https://github.com/z29k/rescriptum/tree/main/examples) contient un exemple
travaillé de chaque format, et c'est le seul endroit où ils sont montrés en train de se
composer ensemble. Deux d'entre eux ont attrapé de vrais bugs — un doctype manquant et un
attribut `pass` non apparié. Gardez-les fonctionnels.

## CI

`.github/workflows/ci.yml`, à chaque push sur `main` et `develop` et à chaque pull request :

| Job | Lance |
|---|---|
| **gates** | `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo test --all-features`, `cargo build --release --no-default-features` |
| **docs** | construit le site public et lance `notabene lint` |
| **audit** | `cargo audit --deny warnings` sur l'arbre de dépendances |
| **cross** | un build ARMv7-musl complet, puis affirme que le binaire est bien `statically linked` |

Le job cross n'est pas redondant. **SQLite est compilé depuis les sources, et `armv7-musl` est
la cible la moins indulgente qui soit livrée** — c'est là qu'une dépendance C casse en premier.
L'attraper sur un push vaut mieux que l'attraper en train de faire une release.

Le job **audit** est l'autre moitié de la règle « ajouter une dépendance exige une raison » :
une raison de l'ajouter n'est pas une raison de la garder. `--deny warnings` échoue aussi sur
un crate non maintenu ou yanké, pas seulement sur une vulnérabilité. Quand quelque chose
apparaît sans correctif, ajoutez `--ignore RUSTSEC-…` avec une ligne expliquant pourquoi,
plutôt que de retirer le drapeau.

Chaque action utilisée est une action officielle `actions/*`, et Zig comme `cargo-audit` sont
installés directement plutôt que via une action tierce. C'est délibéré : cette toolchain
vérifie et lie un binaire que des gens font tourner en root.

Le site de documentation a son propre garde-fou — voir
[le site de documentation](./docs-site.md#le-garde-fou-ci).
