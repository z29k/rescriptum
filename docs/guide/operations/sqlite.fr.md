---
title: Le store SQLite
description: Les mêmes réponses dans une base plutôt qu'un répertoire — comportement identique, mouvement réversible, et aucun logiciel supplémentaire à installer.
sidebar:
  label: Store SQLite
  order: 5
---

# Le store SQLite

Un répertoire de fichiers est le défaut, et c'est la bonne réponse pour une poignée de
machines : greppable, diffable, dans git si vous voulez, sans base à faire tourner,
sauvegarder ou migrer.

Pour un parc administré par **outillage** plutôt qu'à la main, les mêmes réponses peuvent
vivre dans une base SQLite. Elle est compilée dans le binaire, donc il n'y a toujours rien à
installer.

```console
$ export RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db
$ rescriptum import /srv/answers      # faire passer les fichiers
$ rescriptum check                    # les mêmes vérifications, sur la base
$ rescriptum                          # servir depuis elle
```

## Pourquoi vous le feriez

- **L'[API d'administration](./admin-api.md) en a besoin.** Gérer les réponses en HTTP exige
  la base ; au-dessus de fichiers il y aurait deux façons de changer la même configuration —
  à la main et par le réseau — en concurrence l'une avec l'autre.
- **Les écritures concurrentes sont sûres.** Mode WAL, donc une écriture administrative ne
  bloque jamais une installation en cours.
- **Un seul fichier à sauvegarder**, et il se déplace atomiquement.

## Pourquoi peut-être pas

- **Un répertoire est lisible.** `git log answers/` répond à « qui a changé cette baie et
  pourquoi » ; une base non, à moins que votre outillage ne l'enregistre.
- **Un fichier s'édite avec n'importe quoi.** `vi`, `scp`, un Makefile.
- **C'est 1,2 Mo de binaire** — 2,4 Mo avec SQLite contre 1,3 Mo sans, sur ARMv7. Construisez
  avec `cargo build --no-default-features` si cela compte et que vous n'en avez pas besoin.

## Le comportement est identique

Correspondance, groupes, `extends`, fusion, templating, `render`, `check` — tout cela vit
*au-dessus* du store, qui est délibérément mince : il rend le texte brut des documents et un
jeton de version bon marché, et ne décide de rien.

Ce n'est pas affirmé mais imposé. `tests/stores.rs` fait tourner **chaque cas de comportement
deux fois**, une fois par store, et exige le résultat identique. Un nouveau comportement a sa
place dans cette suite, pas dans un test propre à un store.

## Passer de l'un à l'autre

```console
$ rescriptum import /srv/answers      # répertoire → store configuré
$ rescriptum export /tmp/backup       # store configuré → répertoire
```

```console
$ RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db rescriptum import examples
copying files:examples -> sqlite:/srv/answers.db
  10 group(s), 8 machine(s)
  ok — now run `check` against the target
```

**L'aller-retour est identique octet pour octet.** Importez un répertoire, réexportez-le, et
`diff -r` ne signale rien — commentaires, mise en forme et tout le reste. C'est ce qui rend
la base sûre à adopter *et* sûre à quitter, et cela vaut la peine de rester vrai.

Les deux sens lancent `check` sur votre initiative plutôt qu'automatiquement ; la sortie
ci-dessus vous le dit.

## Versions de schéma

La base porte une version de schéma (`user_version`). Il n'y en a qu'une pour l'instant, et
rien n'a été publié sous une plus ancienne : il n'y a donc rien à migrer.

Ce à quoi sert la version, c'est l'autre sens : un binaire **plus ancien** refuse d'ouvrir
une base écrite par un plus récent plutôt que de deviner ce qui a changé.

```
database schema is version 2, this binary understands 1
```

Un retour arrière au-delà d'un futur changement de schéma a donc besoin de l'export d'avant
la mise à jour, ou d'un binaire assez récent pour lire la base. Gardez un `export` sous la
main quand vous en franchissez un.

## Notes d'exploitation

- **`version()` est un atomique en mémoire**, pas une requête, parce qu'il est appelé à chaque
  requête HTTP. Un changement fait par un *autre* processus est rattrapé par le filet de
  rechargement d'une seconde.
- **Le fichier de base et ses compagnons `-wal`/`-shm`** doivent tous être accessibles en
  écriture, et appartiennent tous à la même sauvegarde.
- **`RESCRIPTUM_DB_PATH` vaut par défaut `/srv/answers.db`**, voisin du répertoire de
  réponses par défaut. La base contient le même contenu curé, pas de l'état d'exécution :
  elle a sa place dans le même arbre.
- **Le répertoire parent est créé** s'il n'existe pas.

## Voir aussi

- [L'API d'administration](./admin-api.md) — la raison pour laquelle la plupart des gens
  activent ceci.
- [Comment les stores sont construits](../../development/stores.md) — le trait, et pourquoi
  il est mince.
