---
title: Ligne de commande
description: Chaque sous-commande et chaque option, et ce que signifie chaque code de sortie.
sidebar:
  label: Ligne de commande
  order: 4
---

# Ligne de commande

Sans argument, `rescriptum` lance le serveur. Tout le reste est une sous-commande.

| Commande | Rôle |
|---|---|
| `rescriptum` | lancer le serveur |
| `rescriptum render <id>` | afficher la réponse que cet identifiant recevrait |
| `rescriptum render --body FICHIER` | …pour un corps de requête capturé |
| `rescriptum render --query Q` | …pour des étiquettes, p. ex. `"mac=aa:bb&serial=7ABC1"` |
| `rescriptum check` | rendre tout le store configuré et signaler ce qui casse |
| `rescriptum import <dir>` | copier un répertoire de documents dans le store configuré |
| `rescriptum export <dir>` | écrire le store configuré comme un répertoire de documents |
| `rescriptum --help` | usage et variables d'environnement |

Toutes lisent les mêmes [variables d'environnement](./configuration.md), dont
[`RESCRIPTUM_ENV_FILE`](./configuration.md#le-fichier-denvironnement) — résolu en premier,
donc un fichier illisible arrête toute commande ayant besoin de la configuration. `--help`
et `--version` répondent avant sa lecture, parce que ce sont les commandes qu'on lance
quand quelque chose ne va pas. Il n'y a pas d'options globales.

## `render`

```console
$ rescriptum render 98:fa:9b:50:d8:10
$ rescriptum render --query "serial=7ABC123&mac=98:fa:9b:50:d8:10"
$ rescriptum render --query "path=/rhel/ks&serial=7ABC123"
$ rescriptum render --body /var/log/rescriptum-captures/2026…-0000.body
```

| Forme | Faits fournis |
|---|---|
| `<id>` | l'identifiant comme botte de foin, et rien d'autre — assez pour correspondre par nom, pas assez pour un sélecteur sur `serial` |
| `--query "k=v&k2=v2"` | ces étiquettes, décodées. `path=` fournit aussi `file` et `segment`, et contraint le format comme le ferait une vraie URL |
| `--body FICHIER` | le fichier verbatim : botte de foin, plus le JSON aplati s'il parse comme du JSON |

- Le **document** part sur **stdout** ; la ligne `# format=… machine=… group=…` expliquant
  comment il a été obtenu part sur **stderr**. Donc `render … > answer.toml` ne donne que le
  document.
- Les problèmes de chargement sont d'abord affichés comme lignes `warning:`.
- Sortie **0** quand quelque chose s'est résolu, **1** quand rien ne s'appliquait (le serveur
  aurait renvoyé un `404`) ou que le rendu a échoué.

## `check`

```console
$ rescriptum check
```

Signale les problèmes de chargement, rend chaque document machine et chaque membre de groupe,
nomme les groupes qui sélectionnent sur un bloc `match` (qu'il ne peut pas essayer sans vraie
requête), et appelle le validateur de l'installateur là où il est dans le PATH.

Sortie **0** quand tout se rend, **1** dès que quelque chose a échoué — il tombe donc tel quel
dans une CI. Voir [validation](../answers/validating.md).

## `import` / `export`

```console
$ RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db rescriptum import /srv/answers
$ RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db rescriptum export /tmp/backup
```

`import` lit un **répertoire** et écrit dans le store configuré ; `export` fait l'inverse.
L'aller-retour est identique octet pour octet. Aucun des deux ne lance `check` pour vous — la
sortie vous le dit.

## Codes de sortie

| Code | Signifie |
|---|---|
| `0` | succès |
| `1` | la commande a échoué — rien ne s'est résolu, un document ne parse pas, le store n'a pas pu être ouvert |

Le serveur lui-même sort en `0` sur `SIGTERM` ou Ctrl-C, et en `1` s'il ne peut pas binder ou
ouvrir le store.
