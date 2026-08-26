---
title: Installation
description: Téléchargez un binaire ou construisez-en un — puis lancez-le. Il n'y a pas d'étape d'installation, pas de runtime, pas de conteneur.
sidebar:
  label: Installation
  order: 1
---

# Installation

rescriptum est **un binaire autonome**. Pas de runtime à installer, pas
d'interpréteur, pas d'image de conteneur, et rien d'écrit en dehors du répertoire que vous
lui indiquez. Copiez-le quelque part et lancez-le.

## Télécharger une release

Les binaires de chaque cible publiée sont attachés à chaque
[release](https://github.com/z29k/rescriptum/releases), avec une somme SHA-256 à côté.

| Cible | Pour |
|---|---|
| `armv7-unknown-linux-gnueabihf` | Synology DS416j et autres NAS ARMv7 (glibc ≥ 2.17) |
| `aarch64-unknown-linux-musl` | NAS ARM récents, Raspberry Pi |
| `x86_64-unknown-linux-musl` | la plupart des autres hôtes Linux |
| `aarch64-apple-darwin` | développement local, Apple silicon |
| `x86_64-apple-darwin` | développement local, Mac Intel |

```console
$ VERSION=0.1.0 TARGET=x86_64-unknown-linux-musl
$ curl -fsSLO https://github.com/z29k/rescriptum/releases/download/v$VERSION/rescriptum-$VERSION-$TARGET.tar.gz
$ curl -fsSLO https://github.com/z29k/rescriptum/releases/download/v$VERSION/rescriptum-$VERSION-$TARGET.tar.gz.sha256
$ shasum -a 256 -c rescriptum-$VERSION-$TARGET.tar.gz.sha256
$ tar xzf rescriptum-$VERSION-$TARGET.tar.gz
$ sudo install -m755 rescriptum-$VERSION-$TARGET/rescriptum /usr/local/bin/
```

Vérifiez la somme. Ce binaire tourne en root sur du matériel que vous êtes sur le point
d'installer, ce qui représente à peu près toute la confiance qu'un programme puisse
obtenir.

## Sur un Synology

Prenez plutôt le `.spk` de votre modèle — `rescriptum-<version>-armv7.spk` pour le DS416j et
les autres machines `armada38x`, `-x86_64.spk` pour tous les modèles Intel — et installez-le
par **Package Center → Installation manuelle**. Il crée le dossier partagé, enregistre le
port auprès du pare-feu, lie le CLI dans le `PATH` et démarre au boot. Les détails, et ce
qu'il ne fait délibérément pas pour vous, sont sur la
[page Synology](./operations/synology.md).

Les builds Linux sont liés à musl statiquement — sauf `armv7`, qui vise la glibc 2.17
parce que musl 1.2 ne peut pas tourner sur les noyaux 3.10 de Synology (voir la
[page de build](../development/building.md#pourquoi-armv7-est-la-seule-cible-qui-ne-soit-pas-musl)) :

```console
$ file /usr/local/bin/rescriptum
ELF 64-bit LSB executable, x86-64, ... statically linked, stripped
```

## Ou le construire

Un build natif ne demande qu'une toolchain Rust :

```console
$ git clone https://github.com/z29k/rescriptum && cd rescriptum
$ ./build.sh
```

La compilation croisée pour le NAS demande
[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) et Zig, qui remplacent une
toolchain croisée complète. La [page de build](../development/building.md) donne les
détails, y compris ce qu'il faut vérifier selon la cible : que les builds musl sont bien
statiques, et que le build armv7 ne réclame pas une glibc plus récente que celle du NAS —
les deux échouent au moment de l'exec, sur la machine, et non au build sur votre portable.

## Le lancer

```console
$ mkdir -p /srv/answers
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers rescriptum
2026-08-22T18:00:00Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:/srv/answers workers=8 max_conn=2048 timeout=10s
2026-08-22T18:00:00Z - warning: /srv/answers does not exist yet — every request will 404 until it does
```

La ligne de démarrage mérite d'être lue plutôt que défilée :

| Champ | Signification |
|---|---|
| `listening on` | l'adresse réellement bindée, pas celle demandée — avec `:0` elles diffèrent |
| `store=` | `files:<dir>` ou `sqlite:<path>`, pour qu'un store mal configuré saute aux yeux |
| `workers=` | threads du runtime, nombre de CPU par défaut. **Pas** une limite de concurrence |
| `max_conn=` | connexions en vol avant que le serveur ne délestage en `503` |
| `timeout=` | délai de lecture des en-têtes **et** échéance de la connexion entière |

Tout ce qui cloche dans le jeu de réponses — un groupe qui étend un groupe inexistant, un
document qui ne parse pas — est aussi signalé ici, une fois, au démarrage. C'est également
signalé par [`rescriptum check`](./answers/validating.md), qui est le meilleur endroit pour
l'apprendre.

Confirmez qu'il est vivant :

```console
$ curl http://localhost:8000/health
OK
```

`GET /health` est le seul endpoint qui n'exige jamais de jeton et n'est jamais limité en
débit, donc une supervision continue de fonctionner même pendant que le serveur refuse tout
le reste.

## Où il regarde par défaut

`RESCRIPTUM_ANSWERS_DIR` vaut par défaut **`/srv/answers`**, et `RESCRIPTUM_DB_PATH`
`/srv/answers.db`. `/srv` est l'endroit où la norme de hiérarchie des fichiers range les
données servies par le système, ce qu'elles sont. Rien ne crée le répertoire pour vous ; la
ligne de démarrage le signale s'il manque.

Tout se configure par l'environnement ; il n'y a pas de format de configuration à
apprendre ni de ligne de commande à se tromper. Si vous n'avez nulle part où mettre un
jeton — DSM 7, par exemple — `RESCRIPTUM_ENV_FILE` nomme un fichier contenant les mêmes
variables. La liste complète est dans la
[référence de configuration](./reference/configuration.md).

## Ensuite

- [Servir sa première réponse](./quickstart.md) — une vraie machine recevant un vrai
  document.
- [Déploiement](./operations/deployment.md) — systemd, ou le planificateur de tâches DSM.
