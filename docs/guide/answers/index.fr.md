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

**Un répertoire par identité.** Une machine est un répertoire nommé d'après elle, qui
contient un document par système d'exploitation :

```
answers/
├── 98fa9b50d810/           une machine
│   ├── proxmox.toml            en tant que Proxmox
│   └── debian.preseed          …et le même matériel en tant que Debian
├── aabbccddeeff/           une autre machine
│   └── ubuntu.yaml             en tant qu'Ubuntu
├── default/                quand rien d'autre ne correspond
│   └── proxmox.toml
└── groups/
    ├── rack-a/             partagé par une baie, revendique ses membres
    │   ├── proxmox.toml
    │   └── debian.preseed
    └── rhel-compute/       revendique des machines pour ce qu'elles sont
        └── rhel.ks
```

- **Une machine** est un répertoire nommé d'après elle — une adresse MAC, dans n'importe
  quel style de séparateur — qui porte la configuration de cette machine, ou seulement la
  part qui diffère de son groupe.
- **Un groupe** est un répertoire sous `groups/` et il est partagé. Il revendique des
  machines en les listant dans `members`, ou par un bloc `match` testé contre la requête.
- **`default/`** répond quand rien d'autre ne le fait. Un document par format : un défaut
  TOML ne doit pas répondre à un client qui a demandé du kickstart.

### L'extension décide ; le nom, non

Dans un répertoire, **l'extension est le format et ce qui précède ne veut rien dire**.
`proxmox.toml` et `answer.toml` sont le même document pour le serveur ; le nom est là pour
qui ouvre le dossier. `rescriptum` en écrit des lisibles — `proxmox.toml`, `ubuntu.yaml`,
`debian.preseed`, `boot.ipxe` — et ne renomme jamais les vôtres.

La seule règle qui en découle : **un répertoire contient au plus un document par format**.
Deux `.toml` dans un même répertoire est signalé comme un problème plutôt que tranché,
parce que rien ne pourrait choisir entre eux d'une façon que vous auriez prévue. Deux
formats *différents* ne sont pas un doublon — c'est justement l'intérêt du répertoire.

L'organisation du stockage et l'URL restent délibérément séparées : un dossier peut être
réorganisé, une URL gravée dans une ISO non. Voir
[formats](./formats.md#le-stockage-nest-pas-lurl).

:::note[Migration depuis un répertoire plat]
Les réponses étaient des fichiers à la racine du répertoire : `98fa9b50d810.toml` à côté de
`98fa9b50d810.preseed`. Ils ne sont **plus servis**, et chacun est signalé par son nom avec
son nouveau chemin. `rescriptum migrate` montre ce qu'il déplacerait ; `rescriptum migrate
--apply` les déplace.
:::

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
par matériel, par liste de membres, par nom de répertoire — et ils sont exercés par la suite de
tests :

```console
$ RESCRIPTUM_ANSWERS_DIR=examples rescriptum check
$ RESCRIPTUM_ANSWERS_DIR=examples rescriptum render --query "path=/rhel/ks&serial=7ABC123"
```

C'est le seul endroit où les formats sont montrés en train de se composer ensemble ;
commencez par là si vous ne savez pas à quoi ressemble un vrai fichier.
