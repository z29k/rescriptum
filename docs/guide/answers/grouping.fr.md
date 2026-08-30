---
title: Groupes et fusion
description: Une baie partage un document ; une machine qui diffère ne porte que sa différence. Ce qui fusionne, ce qui remplace, et pourquoi les tableaux remplacent.
sidebar:
  label: Groupement
  order: 3
---

# Groupes et fusion

Une baie de machines partage d'habitude tout sauf ses adresses MAC. L'écrire une fois par
machine est la façon dont la configuration d'un parc dérive. Donc les réponses se composent.

```
answers/
├── groups/
│   ├── base/
│   │   └── proxmox.toml       partagé par tout
│   └── rack-a/
│       └── proxmox.toml       extends = "base" ; members = [ … ]
├── 98-fa-9b-50-d8-10/
│   └── proxmox.toml           les surcharges d'une machine (optionnel)
└── default/
    └── proxmox.toml           seulement quand rien d'autre ne correspond
```

## La partie partagée

```toml
# answers/groups/rack-a/proxmox.toml
members = [
  "98:fa:9b:50:d8:10",
  "98:fa:9b:50:d8:11",
  "98:fa:9b:50:d8:12",
]

[global]
keyboard = "fr"
country  = "fr"
timezone = "Europe/Paris"

[disk-setup]
filesystem = "zfs"
zfs.raid   = "raid1"
disk-list  = ["sda", "sdb"]
```

## La différence

Une machine qui diffère reçoit un document contenant **seulement la différence** :

```toml
# answers/98-fa-9b-50-d8-10/proxmox.toml
[global]
fqdn = "node01.example.com"

[disk-setup]
zfs.raid  = "raid10"                       # celle-ci a quatre disques
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

…et reçoit les deux fusionnés, ses propres valeurs l'emportant :

```console
$ rescriptum render 98:fa:9b:50:d8:10
# format=toml machine=98-fa-9b-50-d8-10 group=rack-a

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
fqdn = "node01.example.com"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid10"
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

## Règles de fusion

| | |
|---|---|
| **Couches** | la chaîne de groupes d'abord, le document machine en dernier — **la machine gagne toujours** |
| **Maps** | fusionnent récursivement, y compris les tables inline et pointées de TOML |
| **Autres valeurs** | remplacées intégralement par la couche supérieure |
| **Tableaux** | **remplacent, ils ne concatènent pas** |
| **Formats texte** | concaténés dans l'ordre des couches — voir [formats](./formats.md#concaténation) |

**Pourquoi les tableaux remplacent.** Concaténer est le choix intuitif jusqu'au moment où il
faut raccourcir une liste. `disk-list = ["sda", "sdb"]` dans un groupe et `["sda"]` dans un
document machine n'a qu'un seul sens raisonnable — *celle-ci n'a qu'un disque* — et la
concaténation ne peut pas l'exprimer. La règle vaut dans tous les formats, pour que vous
n'ayez jamais à vous rappeler dans lequel vous êtes.

## `extends`

Un groupe peut en étendre un autre, ce qui donne une chaîne — ce que toutes les baies
partagent dans un fichier, les différences par baie dans un autre :

```toml
# answers/groups/base/proxmox.toml
[global]
mailto   = "ops@example.com"
timezone = "Europe/Paris"
root-ssh-keys = ["ssh-ed25519 AAAA…REPLACE ops@example.com"]
```

```toml
# answers/groups/rack-a/proxmox.toml
extends = "base"
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11"]

[disk-setup]
filesystem = "zfs"
```

Les couches s'appliquent alors `base` → `rack-a` → document machine.

**`extends` dans un document machine l'emporte sur l'appartenance.** C'est l'échappatoire
pour une machine qui a besoin d'un groupe où elle n'est pas listée :

```toml
# answers/98-fa-9b-50-d8-99/proxmox.toml
extends = "rack-a"          # même si rack-a ne liste pas cette MAC

[global]
fqdn = "spare01.example.com"
```

`extends` se résout **à l'intérieur d'un format** — superposer un preseed sur une base TOML
n'a aucun sens, et la fusion le refuserait de toute façon.

## Seul le premier groupe correspondant s'applique

Si deux groupes revendiquent la même machine, un seul s'applique —
[le plus spécifique](./selection.md#quand-plusieurs-documents-revendiquent-la-même-requête),
les égalités départagées sur le nom trié. Composez avec `extends` plutôt que de compter sur
plusieurs groupes correspondant à la fois : l'ordre entre eux serait arbitraire, et un ordre
arbitraire est la façon dont une machine reçoit discrètement le mauvais schéma de disques.

## Quand un groupe est cassé

Les cycles et les parents manquants sont détectés **à la lecture du store**, signalés une
fois dans le log, et le groupe cassé est **écarté plutôt qu'appliqué à moitié** :

```
2026-08-24T08:43:36Z - warning: group "rack-a": extends unknown group "base"
```

Un groupe cassé n'empêche pas les autres baies de s'installer. Une machine qui
*avait besoin* de ce groupe reçoit un `500` bruyant plutôt qu'une réponse à moitié
construite — servir une configuration dont la base manque installerait la machine à moitié
configurée, et personne ne s'en apercevrait avant qu'elle ne tourne.

`rescriptum check` signale les mêmes problèmes, ce qui est un meilleur endroit pour
l'apprendre que le log à 3 h du matin.

## Le groupement est le chemin rapide

Mesuré à 2 000 machines, 3 000 requêtes à 100 en concurrence :

| Agencement | Débit |
|---|---|
| 2 000 documents machine, aucun groupe | 12 132 req/s |
| un groupe de 2 000 membres, aucun document machine | **13 036 req/s** |
| 2 000 documents machine plus un groupe (une fusion par requête) | 8 816 req/s |

Un groupe sans surcharge machine et sans placeholder est rendu **une fois, à la lecture du
store**, puis servi comme une chaîne préparée. Le cas courant en datacenter ne parse rien par
requête. Ajouter une surcharge par machine coûte une fusion par requête — ça vaut le coup là
où c'est nécessaire, et ça vaut le coup de l'éviter ailleurs.

L'autre moitié du même argument, c'est ce que coûte une *lecture*. Le store entier est relu
au plus une fois par seconde, et avec un répertoire par identité cette lecture ajoute un
`readdir` par machine au fichier qu'elle ouvrait déjà — mesuré à 2 000 machines sur un
M1 Pro : **28 ms avant le changement d'agencement, 63 ms après**. C'est amorti sur une
seconde de requêtes dans les deux cas, et les débits ci-dessus n'ont pas bougé de façon
mesurable ; mais un groupe qui dispense d'un répertoire par machine évite ce coût aussi.

## Ensuite

- [Templating](./templating.md) — `{{ serial }}` supprime la dernière raison d'avoir un
  répertoire par machine.
- [Validation](./validating.md) — une réponse fusionnée est un document que personne n'a
  écrit ; regardez-le avant qu'une baie ne le fasse.
