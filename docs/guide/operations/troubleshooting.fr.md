---
title: Dépannage
description: La ligne de log est tout le diagnostic disponible. Ce que chaque échec signifie, et comment le reproduire hors ligne.
sidebar:
  label: Dépannage
  order: 7
---

# Dépannage

Quand une installation PXE ne démarre pas, le log est le seul diagnostic dont dispose qui que
ce soit — il est donc délibérément ennuyeux et greppable : **une ligne par requête, sur
stderr**. Les deux moitiés se règlent :
[`RESCRIPTUM_LOG`](../reference/configuration.md#journalisation) jette les requêtes qui ont
abouti, et `RESCRIPTUM_LOG_FILE` envoie les lignes dans un fichier.

```
2026-08-24T08:43:36Z - rescriptum 0.1.0 listening on 127.0.0.1:8999 — store=files:answers workers=10 max_conn=2048 timeout=10s
2026-08-24T08:43:37Z 127.0.0.1:61720 GET /health 200
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=example-rack bytes=431
2026-08-24T08:43:37Z 127.0.0.1:61722 GET /rhel/ks?serial=7ABC123 body=0 200 format=text group=rhel-compute bytes=747
2026-08-24T08:43:37Z 127.0.0.1:61723 POST /answer body=27 404 no answer file applies
```

## Lire une ligne

```
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=example-rack bytes=431
└─ horodatage UTC    └─ pair         └─ requête      └─ corps  └─ statut
                                                                  └─ comment la réponse a été composée  └─ octets envoyés
```

Les lignes portant `-` à la place d'une adresse de pair sont au niveau serveur : démarrage,
échecs d'accept, délestage, problèmes de chargement du jeu de réponses.

`format=…` nomme la **famille** (`toml`, `yaml`, `json`, `xml`, `text`) plutôt que
l'extension — `ks` et `preseed` remontent tous deux comme `text`.

## Échecs courants

| Symptôme | Cause probable |
|---|---|
| `404 no answer file applies` | Rien n'a revendiqué la requête et il n'y a pas de `default` pour le format demandé. [Capturez le corps](./capture.md) et vérifiez que la MAC y est vraiment |
| `404` sur une URL qui marchait | L'URL nomme maintenant un [alias de format](../reference/formats.md) qui exclut votre document — `/ubuntu/answer` ne servira pas un `.toml` |
| `500 … extends unknown group` | Un document référence un groupe inexistant. Délibéré : servir une configuration dont la base manque installerait la machine à moitié configurée |
| `500` sur une seule machine | Le document de cette machine, ou son groupe, ne parse pas. La raison est sur la même ligne de log. `rescriptum check` le trouve sans attendre que la machine demande |
| `500 template needs {{ … }}` | Un [placeholder](../answers/templating.md) que la requête n'a pas pu remplir. Jamais servi comme chaîne vide, volontairement |
| `401 bad or missing token` | `RESCRIPTUM_ANSWER_TOKEN` est défini mais l'ISO n'a pas été préparée avec le même `--answer-auth-token` |
| `413` | Un corps de plus de 1 Mo, ou un `Content-Length` qui en annonce un. Refusé depuis l'en-tête, avant toute lecture |
| `503` | Plus de connexions simultanées que `RESCRIPTUM_MAX_CONNECTIONS`. Augmentez-le, ou trouvez qui se connecte |
| Réponse servie, installation quand même ratée | Le document est du TOML valide mais pas du *Proxmox* valide. Passez [`render`](../answers/validating.md) dans `validate-answer` |
| L'installateur ne contacte jamais le serveur | L'URL de l'ISO ou un pare-feu, pas ce serveur. `curl http://SERVER:8000/health` depuis le même réseau |

## Le serveur démarre mais tout part en 404

Regardez la ligne de démarrage. Les deux causes habituelles s'annoncent :

```
warning: /srv/answers does not exist yet — every request will 404 until it does
```

```
warning: /srv/answers cannot be read: Permission denied (os error 13) — every request
will 404 until that is fixed; check the directory's owner against the user this server
runs as
```

Le second est ce que vous obtenez quand le répertoire existe mais que le processus ne peut
pas le lister — la cause habituelle est un répertoire créé en root et un serveur tournant
sous quelqu'un d'autre. La question est posée au système de fichiers plutôt que déduite des
bits de permission, donc elle tient compte du propriétaire, du groupe, des ACL et du
montage.

```
… store=files:/srv/answers …
```

— cette seconde est le répertoire de réponses *par défaut*. Si vous en vouliez un autre,
`RESCRIPTUM_ANSWERS_DIR` n'a pas atteint le processus. Une valeur vide ou
composée d'espaces est traitée comme non définie, et un nombre nul ou impossible à parser
retombe sur sa valeur par défaut.

## Le reproduire hors ligne

C'est le chemin le plus rapide entre « une machine a reçu la mauvaise chose » et un correctif :

```console
$ export RESCRIPTUM_CAPTURE_DIR=/var/log/rescriptum-captures   # puis laissez-le échouer une fois de plus
$ rescriptum render --body /var/log/rescriptum-captures/2026…-0000.body
```

`render` résout exactement comme le serveur, donc ce qu'il affiche est ce que cette machine
aurait reçu. Pas besoin de baie. Voir [capturer les requêtes](./capture.md).

Sans capture, répétez depuis l'identité et l'URL :

```console
$ rescriptum render --query "path=/rhel/ks&mac=98:fa:9b:50:d8:10&serial=7ABC123"
```

Ajoutez `path=` — sans lui, la résolution n'est pas contrainte par le format et peut choisir
un document que la vraie URL aurait exclu, ce qui est exactement le bug que vous pourriez être
en train de chasser.

## Vérifier l'ensemble

```console
$ rescriptum check
```

Problèmes de chargement, chaque machine et chaque membre de groupe rendus, et le validateur
de l'installateur lancé là où il est dans le PATH. Détails dans
[validation](../answers/validating.md#check--tout-rendre-signaler-ce-qui-casse).

## Signaler quelque chose

Pour une mauvaise réponse, le rapport utile est **ce que la machine a envoyé et ce qu'elle a
reçu** — le `.body` et le `.meta` d'une capture, plus la ligne de log.

**Nettoyez les hachages de mot de passe et les clés SSH** avant de joindre quoi que ce soit :
la réponse n'est dans le `outcome` de la capture que par son nom, mais si vous collez aussi le
document rendu, il porte de vrais identifiants.

[Ouvrez un ticket](https://github.com/z29k/rescriptum/issues) avec cela et la version tirée
de la ligne de démarrage.
