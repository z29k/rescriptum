---
title: Un document par système d'exploitation
description: L'extension est le format, l'endpoint choisit entre eux, et la même machine peut exister en Proxmox et en Debian à la fois.
sidebar:
  label: Formats
  order: 2
---

# Un document par système d'exploitation

Un installateur qui va chercher une URL attend une chose bien précise en retour. Un client
kickstart veut du kickstart et s'étranglerait avec du TOML. C'est le protocole, pas une
convention que quelqu'un aurait choisie. Donc :

- **l'endpoint déclare le format** — `/rhel/ks` demande du kickstart ;
- **le document le porte comme extension** — `rhel-compute.ks` ;
- **seuls les documents de ce format peuvent répondre.**

## La conséquence qui fait comprendre

**La réponse d'une machine est spécifique au système d'exploitation auquel elle est
destinée.** Donc ceci n'est pas une machine et deux fichiers :

```
answers/
├── 98fa9b50d810.toml       « cette machine, en tant que Proxmox »
└── 98fa9b50d810.preseed    « cette machine, en tant que Debian »
```

C'est un même matériel avec deux réponses, et les deux peuvent exister en même temps. Celle
qu'une requête reçoit dépend de l'URL sur laquelle elle est arrivée — `/proxmox/answer`
obtient le TOML, `/debian/preseed` obtient le preseed. Aucune n'est plus « la » réponse que
l'autre.

En interne, c'est pourquoi un document est indexé par **(identifiant, format)** plutôt que
par identifiant seul.

## Le stockage n'est pas l'URL

Tout vit dans un seul répertoire plat, et c'est délibéré. **Répertoires et lignes de base
sont un espace de recherche** — ils doivent rester libres d'être réorganisés. **Une URL est
un contrat public gravé dans une ISO** — elle ne doit pas bouger parce que quelqu'un a
renommé un dossier. Une conception antérieure faisait du nom de répertoire *le* segment
d'URL et a été écartée pour exactement cette raison.

Quel alias sert quelle extension est dans la
[référence des formats](../reference/formats.md) ; comment en choisir un pour votre média
est dans [préparer les médias d'installation](../iso.md).

## Les formats

| Extension | Pour | Superposition |
|---|---|---|
| `toml` | Proxmox VE | fusion structurelle |
| `yaml`, `yml` | autoinstall Ubuntu, cloud-init | fusion structurelle |
| `json`, `ign` | Ignition, Flatcar, Fedora CoreOS | fusion structurelle |
| `xml`, `autoyast`, `unattend` | AutoYaST, unattend.xml Windows | fusion structurelle, par élément |
| `ks` | kickstart — RHEL, CentOS, Fedora, Alma, Rocky | concaténation |
| `preseed`, `seed` | preseed Debian | concaténation |
| `cfg`, `ipxe` | scripts de boot et autres configurations en lignes | concaténation |

La liste blanche est délibérée : `txt` n'y est **pas**, pour qu'un fichier de notes égaré à
côté de vos réponses ne devienne jamais un candidat.

`.autoyast` et `.unattend` sont du XML sous un nom qui dit lequel, pour qu'un store
contenant à la fois un profil SUSE et un unattend Windows puisse les distinguer. Le `.xml`
simple répond encore aux deux, ce qui va très bien jusqu'au jour où vous avez les deux.

## Fusion structurelle

Pour `toml`, `yaml`, `json` et `xml`, la superposition est une vraie fusion :

- **Les maps fusionnent clé par clé**, récursivement — y compris les tables inline et pointées
  de TOML.
- **Toute autre valeur est remplacée** intégralement par la couche supérieure.
- **Les tableaux remplacent, ils ne concatènent pas.** Concaténer rendrait une liste
  impossible à raccourcir depuis une couche supérieure, et « ce nœud a deux disques, pas
  quatre » doit rester exprimable.

Les détails, avec exemples, sont dans [groupes](./grouping.md#règles-de-fusion).

## Concaténation

Pour `ks`, `preseed`, `cfg`, `seed` et `ipxe`, la superposition est une **concaténation dans
l'ordre des couches**, et le module le dit plutôt que de prétendre le contraire. Une
directive d'une couche supérieure *suit* celle d'une couche inférieure au lieu de la
supprimer.

Savoir si cela équivaut à une surcharge est l'affaire du format cible : la dernière réponse
gagne en preseed, ce n'est pas toujours le cas en kickstart. **Rendez le résultat et
lisez-le** avant de confier une baie à cela.

Une chose à savoir avant d'écrire une dissertation en tête d'un kickstart : **les
commentaires ordinaires sont servis**. Seules les lignes de directive `# answer:` sont
retirées. C'est très bien — kickstart et preseed autorisent tous deux les commentaires —
mais l'installateur verra tout le reste.

## XML

XML apparie les frères par nom d'élément **plus un attribut discriminant** — `name`, `id`,
`key`, `alias` ou `pass`. C'est ce qui rend

```xml
<settings pass="specialize">
  <component name="Microsoft-Windows-Shell-Setup" …>
```

fusionnable : surcharger une `pass` laisse les autres tranquilles, et surcharger un
`component` ne remplace pas tous les autres composants du fichier. Des frères répétés
**sans** attribut discriminant sont traités comme une liste, et le `config:type="list"`
d'AutoYaST est respecté.

Ce qui survit à une fusion : la déclaration `<?xml?>`, le `<!DOCTYPE>`, les espaces de noms
et les attributs. Ce qui n'y survit **pas** : l'indentation d'origine et le placement des
commentaires — la sortie est re-rendue, pas rustinée.

Il ne comprend aucun schéma. Rendez et vérifiez avant de lui confier une baie.

## Où vivent les clés de contrôle

Les [clés de contrôle](./index.md#les-clés-de-contrôle) voyagent dans ce que chaque format
permet, et sont retirées avant que la réponse ne soit envoyée.

**TOML**

```toml
extends = "base"
members = ["98:fa:9b:50:d8:10"]

[match]
product = "PowerEdge R6*"
```

**YAML / JSON** — les mêmes trois, en clés de premier niveau :

```yaml
extends: base
members: ["98:fa:9b:50:d8:10"]
match:
  file: "user-data"
  product: "PowerEdge R6*"
```

**XML** — un élément `<answer-meta>`, avec `extends` en attribut dessus :

```xml
<answer-meta extends="base">
  <member>52:54:00:11:22:33</member>
  <match manufacturer="Dell Inc." product="PowerEdge R6*" />
</answer-meta>
```

**Kickstart, preseed et tout ce qui est en lignes** — des directives `# answer:` (`//`
fonctionne aussi, pour les formats qui commentent ainsi) :

```
# answer: extends base
# answer: member 00:11:22:33:44:55, 00:11:22:33:44:56
# answer: match serial=7ABC* product=PowerEdge*
```

`match` prend des paires `clé=motif` séparées par des espaces, `member` une liste séparée par
des virgules.

## Une réponse, un format

**Toutes les couches d'une même réponse doivent être du même format.** Un document machine
YAML au-dessus d'un groupe TOML est refusé, pas servi à moitié, et `extends` se résout à
l'intérieur d'un format pour la même raison — superposer un preseed sur une base TOML n'a
aucun sens.

Le groupement n'est par ailleurs pas affecté par tout cela : une baie partage un groupe *par
format*, et une machine qui existe en deux systèmes d'exploitation rejoint deux d'entre eux.

`default` suit la même règle — `default.toml` répond à une requête qui a demandé du TOML, et
jamais à une qui a demandé du kickstart.
