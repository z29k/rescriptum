---
title: Travailler sur rescriptum
description: Orientation pour contributeurs — ce que contient le dépôt, comment le lancer, et où vit le raisonnement.
sidebar:
  label: Orientation
  order: 0
---

# Travailler sur rescriptum

rescriptum est une chose petite et ciblée : il détermine quelle configuration d'installation
revient à chaque machine, la compose à partir de couches, et la sert. Environ 4 000 lignes de
Rust, 308 tests, et une courte liste de contraintes qui ne sont pas révisables à la légère.

Cet espace est le *pourquoi*. Le [Guide](../guide/index.md) est le *quoi*.

## Le mettre en route

```bash
git clone https://github.com/z29k/rescriptum && cd rescriptum
cargo test                      # 308 tests
cargo run -- --help
```

Essayez un changement contre les exemples travaillés plutôt que contre les seuls tests — ils
sont le seul endroit où tous les formats sont montrés en train de se composer ensemble :

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- render --query "path=/rhel/ks&serial=7ABC123"
```

Avant d'ouvrir une PR :

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build --release --no-default-features   # le plus petit build doit continuer de marcher
```

Ces quatre-là sont exactement ce que lance la [CI](./testing.md#ci).

## Le dépôt

| Chemin | Contient |
|---|---|
| `src/main.rs` | mise en place du runtime, boucle d'accept, service des connexions, routage, et la moitié bloquante d'une requête |
| `src/lib.rs` | la crate. `main.rs` est un binaire mince par-dessus, pour que le comportement soit testable directement |
| `src/select.rs` | normalisation, correspondance, superposition — [le comportement qui compte](./selection.md) |
| `src/facts.rs` | ce qu'une requête dit de la machine |
| `src/format/` | une interface par format de document ; `xml.rs` porte l'arbre XML |
| `src/merge.rs` | la fusion TOML, utilisée par `format` |
| `src/store/` | d'où viennent les documents : `file.rs`, `sqlite.rs`, derrière un trait mince |
| `src/admin.rs` | l'API d'écriture, et la garantie qu'une écriture ne peut pas casser le parc |
| `src/config.rs` | configuration par l'environnement |
| `src/envfile.rs` | le fichier de valeurs par défaut que nomme `RESCRIPTUM_ENV_FILE` — jamais découvert, seulement nommé |
| `src/capture.rs` | enregistrement de ce que les machines envoient réellement |
| `src/cli.rs` | les sous-commandes `render`, `check`, `import` et `export` |
| `src/log.rs` | une ligne par événement, des horodatages UTC sans crate de date, et les deux réglages au-dessus |
| `tests/` | le vrai binaire sur une socket (`integration`, `admin`, `guards`), sa ligne de commande (`cli`), et la suite de conformité des deux stores (`stores`) |
| `examples/` | un exemple travaillé de chaque format supporté |
| `docs/` | ce site |

**Ne redéclarez jamais un module dans `main.rs`.** Cela compile une seconde copie, fait tourner
chaque test unitaire deux fois, et laisse les deux copies diverger.

## Par où commencer à lire

- **[Les contraintes](./constraints.md)** — d'abord. Elles expliquent l'essentiel de la forme
  du code, et plusieurs ressemblent à des choses qu'on aurait envie d'« améliorer » tant qu'on
  ne sait pas pourquoi elles sont là.
- **[Architecture](./architecture.md)** — la carte des modules et ce qui circule entre eux.
- **[Le cycle de vie d'une requête](./request-lifecycle.md)** — une requête de l'accept à la
  réponse.
- **[Sélection](./selection.md)** — la partie avec le plus de comportement par ligne.
- **[Pièges déjà rencontrés](./traps.md)** — une liste de choses qui ont coûté du temps une
  fois. La lire coûte moins cher que les redécouvrir.

## Conventions

- **Anglais** pour le code, les commentaires, la documentation et les messages de commit.
  Cette documentation existe en français en plus, pas à la place.
- **Un comportement a sa place dans `tests/stores.rs`**, qui fait tourner chaque cas contre
  les deux stores et exige le résultat identique. Un test couvrant un seul store prouve la
  moitié de ce qu'il prétend. Voir [tests](./testing.md).
- **Les tableaux remplacent, ils ne concatènent pas**, dans tous les formats.
- **Échouer bruyamment.** Un groupe manquant, un template impossible à remplir, un document
  qui ne parse pas — tous sont des erreurs avec une raison. Servir une réponse à moitié
  construite installe une machine de travers, et personne ne s'en aperçoit avant qu'elle ne
  tourne.
- **Ajouter une dépendance exige une raison dans le message de commit.** Ce binaire tourne en
  root sur le matériel d'autres gens, et le job `audit` de la CI est l'autre moitié de cette
  règle : une raison de l'ajouter n'est pas une raison de la garder.
- **Commits conventionnels avec un scope** — `feat(http): …`, `fix(select): …`.

## À lire aussi

[`CLAUDE.md`](https://github.com/z29k/rescriptum/blob/main/CLAUDE.md) à la racine du dépôt est
le document d'architecture écrit pour les agents de code. Il recoupe largement cet espace et
c'est le fichier à mettre à jour quand une contrainte change.
