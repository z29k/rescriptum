---
title: Comment une réponse est choisie
description: Trois façons de revendiquer une machine — par nom, par liste de membres, par ce qu'elle est — et la règle déterministe qui tranche les revendications concurrentes.
sidebar:
  label: Sélection
  order: 1
---

# Comment une réponse est choisie

Un document n'est pas *cherché* ; il **revendique** la requête. Trois façons de le faire,
ordonnées par la finesse avec laquelle elles visent une machine.

## 1. Par le nom

Nommez un document d'après l'adresse MAC de la machine :

```
answers/
├── 98-fa-9b-50-d8-10.toml
├── aabbccddeeff.toml
└── default.toml
```

Quand une requête arrive, le serveur passe en minuscules tout ce qu'elle porte et retire
chaque caractère non alphanumérique, fait de même avec le nom de chaque document, et sert le
premier dont le nom apparaît **à l'intérieur** de la requête. Ainsi
`98-fa-9b-50-d8-10.toml`, `98:fa:9b:50:d8:10.toml` et `98fa9b50d810.toml` correspondent tous
à la même machine — vous n'avez jamais à vous soucier du style de séparateur que Proxmox
utilise cette version-ci, ni de la façon dont il structure son JSON.

Cette normalisation est toute l'astuce, et c'est pourquoi cela survit à un changement de
format de corps entre versions de Proxmox : c'est un test de sous-chaîne sur des octets, pas
un schéma.

Rien n'empêche de nommer un document d'après un numéro de série, un code d'inventaire ou un
nom d'hôte. N'importe quelle chaîne apparaissant dans ce que la machine envoie fera l'affaire.

## 2. Par liste de membres

Un groupe revendique un ensemble de machines en les listant :

```toml
# answers/groups/rack-a.toml
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11", "98:fa:9b:50:d8:12"]
```

Les chaînes de `members` sont normalisées exactement comme les noms de fichiers, donc le
style de séparateur n'a pas d'importance ici non plus. Une machine listée n'a besoin
d'aucun document propre à moins d'avoir quelque chose à surcharger — voir
[groupes](./grouping.md).

## 3. Par ce que la machine est

Un bloc `match` revendique une machine par ses propriétés plutôt que par son identité :

```toml
# answers/groups/dell-r620.toml
[match]
manufacturer = "Dell Inc."
product      = "PowerEdge R620"
serial       = "7ABC*"          # * et ? fonctionnent
```

**Tous les critères doivent tenir** pour que le groupe revendique la requête. `*`
correspond à n'importe quelle suite de caractères et `?` à exactement un ; les deux côtés
sont normalisés avant comparaison, donc la casse et le style de séparateur n'ont jamais
d'importance.

Un document machine peut porter un bloc `match` lui aussi — utile pour « quelle que soit la
machine actuellement dans cet emplacement de châssis ».

## Les faits qu'un sélecteur peut tester

Les faits viennent de trois endroits, délibérément étagés du plus structuré au moins
structuré.

### Les paramètres de query

`?mac=…&uuid=…&serial=…` — la façon dont tout installateur autre que Proxmox s'identifie,
parce qu'iPXE substitue les valeurs dans l'URL avant de la chercher. Fiable, clés
arbitraires, aucune devinette.

Trois autres sont synthétisés depuis l'URL elle-même :

| Fait | Vaut |
|---|---|
| `path` | le chemin entier, débarrassé de ses slashes — `rhel/ks` |
| `file` | son dernier segment — `ks`. C'est ce qui distingue le `user-data` de cloud-init de son `meta-data` |
| `segment` | chaque segment, comme valeurs séparées — `rhel` *et* `ks` |

### Un corps JSON POSTé

Quand le corps est réellement du JSON, il est aplati en **à la fois** ses chemins pointés
complets et ses noms de feuilles nus :

```json
{ "dmi": { "system": { "serial": "7ABC123" } } }
```

donne à la fois `dmi.system.serial` et `serial` tout court. **La forme feuille est
l'essentiel.** La documentation de Proxmox avertit elle-même que le contenu de `dmi` « peut
varier énormément selon le système », donc un sélecteur disant *« un champ nommé `serial`,
où qu'il se trouve »* survit à une réorganisation qu'un chemin figé ne supporterait pas.

Les indices de tableau font partie du chemin mais pas du nom de feuille, donc
`network_interfaces.0.mac` est aussi atteignable par `mac` tout court.

Un corps qui n'est pas du JSON n'est pas une erreur — il n'apporte simplement rien d'autre
que la botte de foin.

### Le corps brut

Normalisé en alphanumériques minuscules : la botte de foin de sous-chaînes qui fait
fonctionner la correspondance par nom. Les valeurs de query et les segments de chemin y sont
aussi ajoutés, donc un document nommé d'après une MAC résout que la MAC soit arrivée dans un
corps POST ou dans une query string. Sans cela, un `GET` — qui n'a aucun corps — ne pourrait
jamais correspondre par nom.

## Quand plusieurs documents revendiquent la même requête

La règle est fixe, et un test l'épingle :

1. **Nommer une machine gagne toujours.** Quel que soit le nombre de critères d'un sélecteur,
   une correspondance d'identité le bat — nommer une machine est la chose la plus précise
   qu'on puisse faire.
2. **Entre sélecteurs, plus de critères gagne.** Trois critères satisfaits battent deux ; une
   règle plus délibérée est une règle plus spécifique.
3. **Les égalités se départagent sur le nom trié.** Le premier par ordre alphabétique.

La réponse ne dépend jamais de l'ordre du système de fichiers ni de l'ordre dans lequel les
lignes sont sorties d'une base. matchbox, l'antécédent le plus proche, documente que sa
propre résolution entre groupes concurrents « ne sera pas déterministe ». Celle-ci l'est.

**Seul le premier groupe correspondant s'applique.** La composition s'exprime avec
[`extends`](./grouping.md#extends), pas en fusionnant tous les groupes qui correspondent —
l'ordre entre plusieurs groupes correspondants serait arbitraire, et un ordre arbitraire est
la façon dont une machine reçoit discrètement le mauvais schéma de disques.

## Et si rien ne correspond

`default.<ext>` est servi, s'il en existe un pour le format demandé par l'endpoint. Sinon la
réponse est **404** — journalisée en `no answer file applies`.

## Essayer avant de démarrer quoi que ce soit

`render` résout exactement comme le ferait le serveur, à partir des faits que vous
fournissez :

```console
$ rescriptum render 98:fa:9b:50:d8:10                              # par identité
$ rescriptum render --query "serial=7ABC123&mac=98:fa:9b:50:d8:10" # par étiquette
$ rescriptum render --query "path=/rhel/ks&serial=7ABC123"         # endpoint compris
$ rescriptum render --body captured-request.json                   # un vrai corps capturé
```

Un identifiant nu ne prétend rien sur *quel genre* d'identifiant il est — il remplit la botte
de foin et rien d'autre. C'est assez pour la correspondance par nom, mais un sélecteur sur
`serial` a besoin de `--query "serial=…"` pour avoir quelque chose à tester. `check`
fonctionne pareil, ce qui est pourquoi un template ayant besoin d'un fait propre à la requête
est [signalé comme problème](./templating.md#check-et-les-faits-propres-à-la-requête).

Pour capturer ce que vos machines envoient réellement, voir
[Capturer les requêtes](../operations/capture.md).
