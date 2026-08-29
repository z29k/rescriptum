---
title: Templating
description: Des placeholders remplis depuis la requête, pour qu'un document de groupe couvre cinq cents machines — et pourquoi une valeur manquante est une erreur plutôt qu'une chaîne vide.
sidebar:
  label: Templating
  order: 4
---

# Templating

Le groupement supprime la duplication entre machines d'accord. Le templating supprime la
dernière raison d'écrire un document par machine : les valeurs qui doivent différer.

```toml
# answers/groups/rack-a/proxmox.toml
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11", "…"]

[global]
fqdn = "node-{{ serial }}.example.com"

[network]
filter.ID_NET_NAME_MAC = "*{{ mac }}"
```

Cinq cents machines, un document. Sans cela, un nom d'hôte par machine signifie un document
par machine — et cinq cents documents qui diffèrent d'une ligne chacun.

Les placeholders fonctionnent dans **tous** les formats : TOML, YAML, JSON, XML, kickstart,
preseed.

> Dans les formats structurés, la substitution porte sur des valeurs de chaîne parsées — un
> commentaire n'est donc qu'un commentaire. Dans les formats **en lignes** (`ks`, `preseed`,
> `cfg`, `ipxe`, `seed`), le document est une chaîne opaque : **un placeholder écrit dans un
> commentaire reste un placeholder** et doit quand même se résoudre. Mentionner
> `{{ serial }}` dans une ligne `#` pour l'expliquer au prochain lecteur fera échouer le
> rendu exactement comme un vrai.

## Ce qu'on peut mettre dedans

| Placeholder | Rempli depuis |
|---|---|
| `{{ mac }}`, `{{ serial }}`, `{{ uuid }}`, … | n'importe quel [fait](./selection.md#les-faits-quun-sélecteur-peut-tester) porté par la requête — paramètres de query, et champs d'un corps JSON POSTé par nom de feuille ou chemin complet |
| `{{ dmi.system.serial }}` | le même corps, par son chemin exact |
| `{{ path }}`, `{{ file }}`, `{{ segment }}` | l'URL sur laquelle la requête est arrivée |
| `{{ group }}` | le nom du groupe qui s'est appliqué |
| `{{ machine }}` | l'identifiant du **document machine** qui a matché |

Les espaces à l'intérieur des accolades sont optionnels : `{{serial}}` et `{{ serial }}`
sont identiques.

### `machine` exige un document machine

`{{ machine }}` est l'identifiant du *document* machine qui a matché — il n'est donc
disponible que lorsque la machine a un document à elle. Une machine revendiquée par la liste
`members` d'un groupe, sans répertoire à elle, n'a pas de valeur `machine` et le rendu échoue
avec `template needs {{ machine }}, but this request carries no "machine"`.

Dans un groupe, utilisez plutôt un fait de la requête :

```toml
[global]
fqdn = "node-{{ mac }}.example.com"      # fonctionne pour chaque membre
```

`{{ machine }}` sert à un document machine qui veut se nommer sans répéter sa propre MAC.

## Une valeur manquante est une erreur

Un placeholder que la requête ne peut pas remplir est un **500 avec la raison**, jamais une
chaîne vide :

```console
$ rescriptum render 98:fa:9b:50:d8:10
error: template needs {{ serial }}, but this request carries no "serial"
```

C'est délibéré. Servir `node-.example.com` installe une machine avec un nom d'hôte cassé et
personne ne le remarque avant plus tard — possiblement bien plus tard, sur une machine déjà
en production. Faire échouer l'installation est le résultat le moins coûteux.

Les caractères de contrôle sont refusés pour la même classe de raison : un saut de ligne dans
une valeur kickstart injecterait une directive dans le fichier que l'installateur exécute.

```console
$ rescriptum render --query "mac=aa:bb&serial=$(printf 'a\nb')"
error: value for "serial" contains a control character and will not be substituted
```

## La substitution est sûre vis-à-vis de l'échappement

**La substitution se fait sur des valeurs parsées, jamais sur le texte brut du document.** La
valeur est placée dans le modèle de données du document et le sérialiseur du format l'écrit —
donc c'est le sérialiseur qui fait l'échappement.

Un numéro de série contenant un guillemet ne peut pas casser le TOML dans lequel il atterrit :

```console
$ rescriptum render --query 'mac=aa:bb&serial=a"b'"'"'c<d>e'
[global]
fqdn = """node-a"b'c<d>e.example.com"""
```

L'écrivain TOML a choisi de lui-même une chaîne multi-ligne. La même valeur dans un document
XML revient échappée en entités, et en JSON, échappée en JSON. Un test fait passer
`a"b'c<d>e&f` dans les quatre formats structurés et **reparse la sortie**.

C'est pourquoi le templating peut sans risque être alimenté par une requête que contrôle une
machine que vous n'avez jamais vue.

## `check` et les faits propres à la requête

`rescriptum check` rend chaque machine à partir de sa **seule identité** — il n'a aucune
requête sur laquelle s'appuyer, puisqu'il n'y a pas de requête. Un template ayant besoin de
`serial`, qui n'arrive jamais que dans un corps ou une query string, est donc signalé comme
un problème :

```console
$ rescriptum check
  FAIL group "rack-a" member "98fa9b50d811": template needs {{ serial }}, but this request carries no "serial"
```

C'est honnête — `check` ne peut réellement pas prouver que cette réponse se rend — mais c'est
bruyant pour un jeu qui template délibérément sur des faits de requête. Vérifiez ceux-là avec
`render` et des faits représentatifs :

```console
$ rescriptum render --query "mac=98:fa:9b:50:d8:11&serial=7ABC123"
```

## Le coût

Nul, quand vous ne l'utilisez pas. Un groupe dont la chaîne préparée ne contient pas `{{` est
servi tel quel, sans être parsé par requête — la vérification de présence de placeholders se
fait une fois, à la lecture du store. Le templating ne fait basculer un groupe sur le chemin
« fusion par requête » que pour les documents qui en contiennent réellement un.

## Ensuite

- [Validation](./validating.md) — rendre avec de vrais faits, et vérifier l'ensemble.
- [Capturer les requêtes](../operations/capture.md) — obtenir un vrai corps sur lequel rendre.
