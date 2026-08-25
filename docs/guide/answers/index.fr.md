---
title: Écrire des réponses
description: Sélection, formats, groupes, templating et validation — tout sur les documents que rescriptum sert.
sidebar:
  label: Écrire des réponses
  order: 4
  indexLabel: Vue d'ensemble
---

# Écrire des réponses

Une **réponse** est le document que reçoit un installateur : un `answer.toml` Proxmox, un
`user-data` d'autoinstall Ubuntu, un kickstart, un preseed, une configuration Ignition, un
profil AutoYaST, un `unattend.xml` Windows. Tout le travail de rescriptum consiste à choisir
le bon pour la machine qui demande et à le lui remettre, assemblé à partir du nombre de
couches que vous avez écrites.

## L'agencement

Tout vit dans un seul répertoire plat. Il n'y a pas de dossier par OS, parce que
l'organisation du stockage et l'URL sont délibérément séparées — voir
[formats](./formats.md#le-stockage-nest-pas-lurl).

```
answers/
├── 98fa9b50d810.toml       cette machine, en tant que Proxmox
├── 98fa9b50d810.preseed    …et la même machine, en tant que Debian
├── aabbccddeeff.yaml       une autre machine, Ubuntu
├── default.toml            quand rien d'autre ne correspond
└── groups/
    ├── rack-a.toml         partagé par une baie, revendique ses membres
    ├── base.preseed
    └── rhel-compute.ks     revendique des machines pour ce qu'elles sont
```

- **Un document machine** est nommé d'après la machine — une adresse MAC, dans n'importe quel
  style de séparateur — et porte la configuration de cette machine, ou seulement la part qui
  diffère de son groupe.
- **Un groupe** vit dans `groups/` et est partagé. Il revendique des machines en les listant
  dans `members`, ou par un bloc `match` testé contre la requête.
- **`default.<ext>`** répond quand rien d'autre ne le fait. Un par format : un défaut TOML ne
  doit pas répondre à un client qui a demandé du kickstart.

## Les cinq choses à savoir

| | |
|---|---|
| **[Comment une réponse est choisie](./selection.md)** | Par nom, par liste de membres, ou par ce que la machine *est*. Nommer gagne toujours ; entre sélecteurs, plus de critères gagne ; les égalités se départagent sur le nom trié |
| **[Un document par système d'exploitation](./formats.md)** | L'extension est le format, l'endpoint choisit entre eux, et une machine peut exister en plusieurs systèmes à la fois |
| **[Groupes et fusion](./grouping.md)** | Les couches s'appliquent de la plus basse à la plus haute et la machine gagne toujours. Les maps fusionnent ; **les tableaux remplacent** |
| **[Templating](./templating.md)** | `{{ serial }}` rempli depuis la requête, pour qu'un fichier de groupe couvre une baie |
| **[Validation](./validating.md)** | `render` montre ce qu'une machine recevrait ; `check` rend tout et signale ce qui casse |

## Les clés de contrôle

Trois clés pilotent la résolution et sont **retirées avant que la réponse ne soit envoyée**,
donc l'installateur ne les voit jamais :

| Clé | Rôle |
|---|---|
| `members` | les machines pour lesquelles ce groupe répond |
| `match` | des critères testés contre les faits de la requête |
| `extends` | le groupe au-dessus duquel ce document se superpose |

Elles voyagent dans ce que chaque format permet — clés de premier niveau en TOML, YAML et
JSON, un élément `<answer-meta>` en XML, des directives `# answer:` en kickstart et preseed.
L'écriture par format est dans [formats](./formats.md#où-vivent-les-clés-de-contrôle).

## Exemples travaillés

Le répertoire [`examples/`](https://github.com/z29k/rescriptum/tree/main/examples) du dépôt
contient un exemple commenté de **chaque** format supporté, tous sélectionnés différemment —
par matériel, par liste de membres, par nom de fichier — et ils sont exercés par la suite de
tests :

```console
$ RESCRIPTUM_ANSWERS_DIR=examples rescriptum check
$ RESCRIPTUM_ANSWERS_DIR=examples rescriptum render --query "path=/rhel/ks&serial=7ABC123"
```

C'est le seul endroit où les formats sont montrés en train de se composer ensemble ;
commencez par là si vous ne savez pas à quoi ressemble un vrai fichier.
