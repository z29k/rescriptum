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
| `rescriptum config` | afficher la configuration, et d'où vient chaque valeur |
| `rescriptum config --json` | la même chose, pour un panneau de réglages |
| `rescriptum config --value CLÉ` | une valeur, pour un script — jamais un identifiant |
| `rescriptum config set C=V …` | éditer le fichier que `RESCRIPTUM_ENV_FILE` nomme |
| `rescriptum config unset CLÉ …` | recommenter un réglage dedans |
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

## `config`

La configuration, ce sont des variables d'environnement, et sur un déploiement qui les lit
depuis un fichier — une installation par paquet, surtout — voici comment les voir et les
changer sans ouvrir d'éditeur. C'est aussi ce que
l'[application DSM](../operations/synology.md#lapplication-de-bureau) exécute dessous.

```console
$ rescriptum config
env file: /var/packages/rescriptum/etc/rescriptum.env

  RESCRIPTUM_STORE            files                             default
  RESCRIPTUM_ANSWERS_DIR      /volume1/netboot/answers          file
  RESCRIPTUM_LISTEN_ADDR      0.0.0.0:9000                      environment
  RESCRIPTUM_ADMIN_TOKEN      (set)                             file
```

La troisième colonne est l'essentiel. Le fichier fournit des **valeurs par défaut** et
l'environnement réel l'emporte : une valeur marquée `environment` ne peut donc pas être
changée en éditant le fichier — et `config set` le dit, plutôt que de vous laisser écrire
quelque chose que le serveur en cours continuera d'ignorer.

**Un identifiant n'est jamais affiché**, sous aucune forme de cette commande. Un jeton
apparaît comme `(set)` ou `(not set)` ; `--value` refuse tout net.

```console
$ rescriptum config set RESCRIPTUM_LOG=problems RESCRIPTUM_CAPTURE_DIR=/srv/captures
wrote /var/packages/rescriptum/etc/rescriptum.env
```

L'écriture laisse le fichier tel qu'il est par ailleurs : les commentaires restent, un
réglage est remplacé là où il se trouve, et un réglage commenté est **décommenté sur place**
plutôt qu'ajouté en dessous — ce qui compte quand le commentaire au-dessus est la seule
documentation qu'a le fichier.

Deux refus sont délibérés :

- **Une modification qui laisserait un serveur incapable de démarrer est refusée**, en bloc,
  avant toute écriture. Activer l'API d'administration sans jeton, ou avec un jeton de moins
  de 16 caractères, vous vaut la raison plutôt qu'un prochain démarrage cassé.
- **Une variable mal orthographiée est refusée.** Écrite, elle serait relue comme une
  inconnue et signalée au démarrage suivant, quand plus personne ne fait le lien.

Contrairement à toutes les autres sous-commandes, celle-ci fonctionne quand la configuration
est trop cassée pour démarrer un serveur — un fichier qui ne parse pas, un jeton d'un
caractère trop court. C'est l'état dont on se sert d'elle pour *sortir*.

## `media`

Les médias de démarrage : les images d'installation que ce serveur détient. Chacune de
ces commandes exige `RESCRIPTUM_MEDIA_DIR` ; sans elle, elles le disent et sortent en
`1`. Voir [Servir les médias de démarrage](../operations/media.md).

```console
$ rescriptum media list                    # ce qui est détenu : famille, architecture, version, empreinte
$ rescriptum media add FILE [--sha256 D]   # enregistrer une image déjà dans le répertoire
$ rescriptum media check                   # revérifier chaque empreinte enregistrée
$ rescriptum media ipxe ID                 # imprimer la réponse .ipxe qui démarre une image
$ rescriptum media prepare ID [--url URL]  # une image Proxmox avec son URL de réponse dedans
$ rescriptum media export ID FICHIER       # matérialiser une entrée préparée, pour une clé
```

`media add` prend un fichier **déjà dans le répertoire de médias** — rien n'est
téléchargé et rien n'est copié. Il le hache avec une progression, l'analyse, et écrit un
fichier compagnon `.media` à côté. `--sha256` est vérifié avant tout enregistrement :
un écart n'écrit rien et sort en `1`.

Le code de sortie de `media check` est un contrat, comme celui de `check`. `deploy.sh`
s'y fie.

`media ipxe` imprime sur **stdout** et met tout le reste sur stderr, de sorte que
`rescriptum media ipxe pve-8.4 > groups/rack-a.ipxe` produit un document de réponse
utilisable — ce qu'il est, rien de plus. Il imprime un script, il n'en installe pas.

## `boot`

La moitié démarrage réseau : les chargeurs de TFTP, la configuration DHCP générée, et les
deux scripts qu'une machine exécute. Voir
[Démarrer une machine par le réseau](../operations/netboot.md).

```console
$ rescriptum boot dhcp-snippet [--format F] [--one-loader]
$ rescriptum boot check        # les chargeurs qu'un extrait nomme sont-ils sur le disque ?
$ rescriptum boot bootstrap    # imprimer le script de l'étape deux
$ rescriptum boot menu         # imprimer le menu intégré
```

`--format` vaut `dnsmasq` (par défaut), `isc`, `kea`, `powershell`, `pfsense` ou
`mikrotik`. L'extrait part sur **stdout** et les avertissements sur stderr, de sorte que
`boot dhcp-snippet > dhcpd.conf` produit un fichier incluable tel quel.

Le code de sortie de `boot check` est un contrat, comme celui de `check`. Ce qu'il
attrape est la panne la moins diagnosticable de la chaîne : **un extrait nommant un
chargeur absent du disque échoue silencieusement au niveau de la ROM**, sans rien sur
aucune console. Il signale aussi que le listener média a quitté le port que les chargeurs
distribués ont gravé.

`boot bootstrap` et `boot menu` impriment ce qu'une machine exécutera, pour la même
raison que `render` imprime une réponse : tout ce qu'une baie exécute devrait d'abord
être lisible par un humain.

## Codes de sortie

| Code | Signifie |
|---|---|
| `0` | succès |
| `1` | la commande a échoué — rien ne s'est résolu, un document ne parse pas, le store n'a pas pu être ouvert |

`config` est la seule à avoir un second sens : **`0` dit que la configuration en est une sur
laquelle le serveur démarrerait**, `1` qu'elle ne l'est pas — ou qu'une écriture a été
refusée. Cela la rend utilisable depuis un script, comme `check`.

Le serveur lui-même sort en `0` sur `SIGTERM` ou Ctrl-C, et en `1` s'il ne peut pas binder ou
ouvrir le store.
