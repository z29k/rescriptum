---
title: Les stores
description: Deux backends derrière un trait à deux méthodes, et la suite de conformité qui les empêche de diverger.
sidebar:
  label: Stores
  order: 6
---

# Les stores

Les réponses viennent soit d'un répertoire de documents (`RESCRIPTUM_STORE=files`, le défaut),
soit d'une base SQLite (`RESCRIPTUM_STORE=sqlite`), au choix à l'exécution.

## Le trait est délibérément mince

```rust
pub trait Store: Send + Sync {
    fn version(&self) -> Version;               // Option<String>, bon marché par requête
    fn snapshot(&self) -> io::Result<Snapshot>; // seulement quand la version a bougé
    fn describe(&self) -> String;
}
```

Un `Snapshot`, c'est du texte brut de documents et rien d'autre : `RawMachine`, `RawGroup`,
`RawDefault`, portant chacun un identifiant, un format et un corps.

**Chaque décision vit au-dessus de cela.** Correspondance, chaînes `extends`, fusion, rendu,
`check` — tout dans `select.rs` et `merge.rs`, partagé. Gardez-le ainsi : dès qu'un backend se
met à décider du comportement, les deux divergent et la suite de conformité cesse de pouvoir
prouver le contraire.

La moitié écriture est séparée, parce que servir des réponses n'en a jamais besoin :

```rust
pub trait StoreWrite: Store {
    fn put_machine(&self, id: &str, format: &str, body: &str) -> io::Result<()>;
    fn delete_machine(&self, id: &str, format: &str) -> io::Result<bool>;
    fn put_group(&self, name: &str, format: &str, body: &str) -> io::Result<()>;
    fn delete_group(&self, name: &str, format: &str) -> io::Result<bool>;
    fn put_default(&self, format: &str, body: &str) -> io::Result<()>;
    fn delete_default(&self, format: &str) -> io::Result<bool>;
}
```

**Chaque opération nomme un format.** Un document est indexé par *ce à quoi il sert* — une
machine *et* un système d'exploitation.

> Un `put` antérieur supprimait les autres formats d'un radical, pour éviter « deux réponses
> pour une machine ». C'était le mauvais modèle : ce sont les réponses de cette machine pour
> **deux systèmes d'exploitation**, et les deux sont censées exister. Voir
> [pièges](./traps.md).

## `tests/stores.rs` est la garantie

Chaque cas de comportement tourne **deux fois**, une par store, et affirme le résultat
identique. 35 cas au dernier compte.

**Un nouveau comportement a sa place là, pas dans un test propre à un store.** Un test qui
couvre un seul backend prouve la moitié de ce qu'il prétend — et la moitié qu'il ne couvre pas
est exactement là où se cache une divergence.

## Le store fichiers

Un répertoire plat, plus `groups/`. `version()` est le mtime du répertoire :

```rust
fs::metadata(&self.dir).ok()
    .and_then(|m| m.modified().ok())
    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
    .map(|d| d.as_nanos().to_string())
```

Un `stat` remplace tout un parcours de répertoire — voir
[le cache du listing](./selection.md#le-cache-du-listing).

> Le mtime du répertoire bouge quand un fichier est **ajouté ou supprimé**, pas quand un
> fichier est **édité**. Le filet de rechargement d'une seconde est ce qui couvre l'édition,
> et un test d'intégration couvre exactement cela.

**Les écritures passent par un fichier temporaire plus un `rename`**, atomique dans un
répertoire sur POSIX, pour qu'un lecteur ne rencontre jamais une réponse à moitié écrite. Le
nom temporaire porte l'identifiant du processus, et il est supprimé si le rename échoue. Un
test affirme qu'aucun fichier `.tmp` ne survit.

**La lecture utilise `DirEntry::file_type()`, pas `fs::metadata`.** Le type de fichier revient
gratuitement avec le `readdir` sur Unix ; seul un lien symbolique a besoin du `stat` pour être
résolu. Cela seul valait 65 % à 2 000 fichiers, *avant* que le cache ne soit ajouté.

## Le store SQLite

`rusqlite` avec la feature `bundled` — SQLite est compilé depuis les sources dans le binaire,
donc il n'y a rien à installer. Il se compile en croisé vers `armv7-musl` sous zigbuild ; la
CI construit cette cible à chaque push précisément parce que c'est là qu'une dépendance C
casse en premier.

**Mode WAL**, pour que l'API d'administration ne bloque jamais une installation en cours.

**`version()` lit un atomique en mémoire**, pas la base :

```rust
Some(self.revision.load(Ordering::Relaxed).to_string())
```

Elle est appelée à chaque requête, et une requête SQL par requête HTTP annulerait l'intérêt du
cache. La conséquence est qu'un changement fait par un **autre processus** ne la bouge pas —
c'est le [filet de rechargement](./selection.md#le-cache-du-listing) qui l'attrape.

**Les versions de schéma** vivent dans `PRAGMA user_version`. Il n'y en a qu'une, et rien
n'a été publié sous une plus ancienne, donc `migrate()` n'a aucune étape : il refuse une base
venue du futur, crée le schéma quand la version vaut `0`, et l'estampille. Les formes par
lesquelles ce schéma est passé pendant son écriture ne sont jamais sorties du dépôt, et
porter des migrations depuis elles reviendrait à porter du code qui ne peut pas s'exécuter.

Ce à quoi sert la version, c'est le sens du retour arrière :

```
database schema is version 2, this binary understands 1
```

Refusé plutôt que deviné, parce qu'une base écrite par un binaire plus récent peut porter des
colonnes que celui-ci ignorerait silencieusement — et ignorer silencieusement une partie d'un
jeu de réponses, c'est ainsi qu'une machine s'installe de travers.

## `import` / `export`

```console
$ rescriptum import <dir>    # répertoire → store configuré
$ rescriptum export <dir>    # store configuré → répertoire
```

Les deux passent par `Snapshot`, donc ils partagent toutes les règles. **L'aller-retour est
identique octet pour octet** — importez un répertoire, réexportez-le, `diff -r` ne signale
rien. C'est ce qui rend la base sûre à adopter *et* sûre à quitter, et cela vaut la peine de
rester vrai.

## Les identifiants deviennent des noms de fichiers

```rust
pub fn valid_id(id: &str) -> bool   // lettres, chiffres, - _ . : et aucun séparateur de chemin
```

Imposé à la frontière de l'API d'administration **et** dans les deux stores. Le store est la
couche qui transforme un identifiant en chemin, donc c'est la couche qui ne doit pas être
trompée — ne vérifier qu'à la frontière ferait dépendre le garde-fou du fait que tout appelant
futur s'en souvienne.

`valid_format` est l'équivalent pour les extensions : un document dans un format que personne
ne peut lire n'atteint jamais le store.

## La feature cargo `sqlite`

Activée par défaut, et retirable :

| Build | Taille ARMv7 |
|---|---|
| par défaut | 2 103 456 octets |
| `--no-default-features` | 944 928 octets |

La retirer retire aussi l'API d'administration, qui a besoin de la base. La CI construit
`--release --no-default-features` à chaque push pour que le plus petit build ne pourrisse pas
sans qu'on le remarque.
