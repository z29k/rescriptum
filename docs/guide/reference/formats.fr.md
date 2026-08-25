---
title: Formats et alias d'endpoint
description: Quelle extension est quel format, quel segment d'URL le demande, et les deux pièges de la table d'alias.
sidebar:
  label: Formats et alias
  order: 3
---

# Formats et alias d'endpoint

Trois tables. La version narrative est dans
[un document par système d'exploitation](../answers/formats.md).

## Extensions de document

La liste blanche des extensions que rescriptum ramassera dans un store. Tout le reste est
ignoré — `txt` n'y est délibérément **pas**, pour qu'un fichier de notes égaré à côté de vos
réponses ne devienne jamais un candidat.

| Extension | Famille | Superposition | Content-Type |
|---|---|---|---|
| `toml` | TOML | fusion structurelle | `text/plain; charset=utf-8` |
| `yaml`, `yml` | YAML | fusion structurelle | `text/yaml; charset=utf-8` |
| `json`, `ign` | JSON | fusion structurelle | `application/json` |
| `xml`, `autoyast`, `unattend` | XML | fusion structurelle, par élément | `application/xml; charset=utf-8` |
| `ks`, `cfg`, `preseed`, `seed`, `ipxe` | texte | concaténation dans l'ordre des couches | `text/plain; charset=utf-8` |

La **famille** est ce que remonte le champ `format=` de la ligne de log, donc `ks` et
`preseed` apparaissent tous deux en `format=text`. L'**extension** est ce sur quoi un endpoint
filtre, et ce dont `check` a besoin pour choisir le bon validateur.

## Alias d'endpoint

Un segment de chemin nommant l'un de ceux-ci restreint la réponse aux documents portant les
extensions listées. **N'importe quel** segment du chemin peut le nommer, donc `/rhel/ks`,
`/ks` et `/provision/rhel/node.cfg` restreignent tous au kickstart.

| Segment | Sert | Usage typique |
|---|---|---|
| `proxmox`, `pve`, `toml` | `.toml` | Proxmox VE |
| `debian`, `preseed` | `.preseed`, `.seed` | preseed Debian |
| `rhel`, `centos`, `fedora`, `alma`, `rocky`, `kickstart`, `ks` | `.ks` | kickstart |
| `ubuntu`, `autoinstall`, `cloudinit`, `nocloud`, `yaml`, `yml` | `.yaml`, `.yml` | autoinstall Ubuntu, cloud-init |
| `flatcar`, `coreos`, `ignition`, `ign` | `.ign`, `.json` | Ignition |
| `suse`, `opensuse`, `autoyast` | `.autoyast`, `.xml` | AutoYaST |
| `windows`, `unattend` | `.unattend`, `.xml` | unattend.xml Windows |
| `json` | `.json`, `.ign` | |
| `xml` | `.xml` | |
| `cfg` | `.cfg` | |
| `ipxe` | `.ipxe` | |

**Un segment n'en nommant aucun ne contraint rien**, ce qui est pourquoi `/answer` continue
de fonctionner exactement comme avant.

### Deux pièges dans cette table

- **Le filtrage porte sur l'extension, pas sur la famille.** `.ks` et `.preseed` sont tous
  deux des documents texte ; filtrer par famille laisserait un preseed répondre à `/rhel/ks`.
- **`seed` n'est délibérément pas un alias.** `s=http://server/seed/` est une URL de seed
  NoCloud parfaitement ordinaire, et elle sert du YAML. Un alias doit être assez spécifique
  pour que personne ne l'atteigne par accident. (L'*extension* `.seed` existe toujours, et
  `/debian/` la sert.)

## Clés de contrôle, par format

Retirées avant que la réponse ne soit envoyée.

| Format | Écriture |
|---|---|
| TOML | `extends = "base"`, `members = […]`, table `[match]`, au premier niveau |
| YAML | `extends:`, `members:`, `match:` au premier niveau |
| JSON | `"extends"`, `"members"`, `"match"` au premier niveau |
| XML | `<answer-meta extends="base"><member>…</member><match k="v" /></answer-meta>` |
| Texte | `# answer: extends <nom>` · `# answer: member a, b` · `# answer: match k=v k2=v2` |

Les directives texte acceptent aussi `//` comme marqueur de commentaire. `match` prend des
paires `clé=motif` séparées par des espaces ; `member` une liste séparée par des virgules.
Les commentaires ordinaires d'un document texte sont **servis** — seules les lignes
`# answer:` sont retirées.

## Sémantique de fusion

| | Formats structurés | Formats texte |
|---|---|---|
| Maps / objets / éléments | fusionnent récursivement | — |
| Scalaires | la couche supérieure remplace | — |
| Tableaux / listes | **remplacent**, jamais de concaténation | — |
| Document entier | — | concaténé dans l'ordre des couches |

XML apparie les frères par nom d'élément plus un attribut discriminant — `name`, `id`, `key`,
`alias`, `pass` — et respecte `config:type="list"`. Déclarations, doctypes, espaces de noms et
attributs survivent à une fusion ; l'indentation d'origine et le placement des commentaires,
non.

## Validateurs que `check` peut appeler

| Format | Outil | Invoqué comme |
|---|---|---|
| `toml` | `proxmox-auto-install-assistant` | `validate-answer <fichier>` |
| `xml`, `autoyast`, `unattend` | `xmllint` | `--noout <fichier>` |
| `ks` | `ksvalidator` | `<fichier>` |
| tout le reste | — | aucun n'existe |

Un outil absent du PATH est signalé une fois comme note, jamais comme un échec.
