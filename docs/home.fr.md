# rescriptum

*Un serveur HTTP pour compiler et rendre les fichiers de configuration des installations
automatisées d'OS.*

**Installer un parc sans écrire un fichier par machine — une seule config, et seulement les
différences.** Il répond à n'importe quel installateur automatisé : Proxmox, Debian, RHEL,
Ubuntu, Flatcar, SUSE, Windows. Il reconnaît la machine à son adresse MAC, son numéro de
série ou son inventaire matériel, empile les couches qui la concernent et renvoie le
résultat. Ce qu'une machine s'apprête à recevoir, vous pouvez le lire avant de l'allumer.

```mermaid
flowchart LR
  M["N'importe quelle machine<br/>même image, même URL"]
  M -->|"MAC · numéro de série · DMI"| R["rescriptum"]
  R --> B["groups/base/"]
  R --> A["groups/rack-a/"]
  R --> H["98fa9b50d810/"]
  B --> G["fusion<br/>la machine gagne toujours"]
  A --> G
  H --> G
  G -->|"un document, pour cette machine-là"| M
```

Toutes les machines démarrent sur la même image et interrogent le même serveur : l'adresse
gravée dans l'image ne peut donc pas être ce qui les distingue. Ce qui les distingue, c'est
ce qu'elles disent en demandant — et les installateurs demandent de deux façons, toutes deux
prises en charge ici, sur n'importe quel chemin :

- **Ils POSTent ce qu'ils ont trouvé.** Proxmox VE, depuis la 8.2, envoie un inventaire JSON
  — cartes réseau et adresses MAC, disques, DMI — et attend le fichier de réponse dans le
  corps de la réponse HTTP. C'est pour cela qu'un serveur de fichiers statiques ne peut pas
  faire ce travail : la réponse dépend de la requête.
- **Ils GETent avec leur identité dans la query string.** Kickstart, preseed, autoinstall
  Ubuntu, Ignition, AutoYaST : iPXE substitue la MAC ou le numéro de série dans l'URL avant
  d'aller la chercher.

```console
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers rescriptum
2026-08-24T08:43:36Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:/srv/answers workers=8 max_conn=2048 timeout=10s
2026-08-24T08:43:37Z 10.0.0.42:51234 POST /answer body=1876 200 format=toml machine=98fa9b50d810 group=rack-a bytes=431
```

Un binaire statique, sans runtime, sans conteneur — aussi à l'aise sur un NAS ARM de 512 Mo
que sur un hôte de datacenter encaissant une rafale de provisioning.

## Trente secondes

```console
$ mkdir -p answers/groups/rack-a
$ cat > answers/groups/rack-a/proxmox.toml <<'TOML'
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11"]

[global]
keyboard = "fr"
timezone = "Europe/Paris"

[disk-setup]
filesystem = "zfs"
zfs.raid   = "raid1"
TOML

$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum render 98:fa:9b:50:d8:10
# format=toml group=rack-a
[global]
keyboard = "fr"
timezone = "Europe/Paris"
…
```

Voilà une baie **en tant que Proxmox**. Le même répertoire contient `groups/rack-a/rhel.ks`
pour les nœuds RHEL et `groups/rack-a/debian.preseed` pour les Debian — même répertoire, autre
extension. Un
document est indexé par *(machine, format)*, donc une machine peut être plusieurs systèmes
d'exploitation à la fois et c'est l'URL qui tranche.

Puis pointez ce que vous installez sur **son URL** — un seul serveur leur répond à tous :

| Vous installez | Pointez-le sur | Sert |
|---|---|---|
| Proxmox VE | `--url http://SERVER:8000/proxmox/answer` | `.toml` |
| RHEL · CentOS · Fedora · Alma · Rocky | `inst.ks=http://SERVER:8000/rhel/ks?mac=${net0/mac}` | `.ks` |
| Debian | `url=http://SERVER:8000/debian/preseed?mac=${net0/mac}` | `.preseed` |
| Ubuntu | `ds=nocloud-net;s=http://SERVER:8000/ubuntu/?mac=${net0/mac}` | `.yaml` |
| Flatcar · Fedora CoreOS | `ignition.config.url=http://SERVER:8000/flatcar/config` | `.ign` |
| openSUSE · SLES | `autoyast=http://SERVER:8000/suse/profile` | `.autoyast` |
| Windows | votre propre outillage, depuis `http://SERVER:8000/windows/unattend` | `.unattend` |
| tout ce qui est en lignes | `http://SERVER:8000/cfg/…`, `/ipxe/…` | `.cfg`, `.ipxe` |

## Commencer ici

- **[Ce qu'est rescriptum](./guide/index.md)** — le problème qu'il résout et la forme de la
  solution, en cinq minutes.
- **[Installation](./guide/install.md)** puis **[servir sa première réponse](./guide/quickstart.md)** —
  du binaire téléchargé à une machine qui reçoit sa propre configuration.
- **[Préparer les médias d'installation](./guide/iso.md)** — l'URL à graver dans chaque ISO,
  par système d'exploitation.

## Écrire des réponses

- **[Comment une réponse est choisie](./guide/answers/selection.md)** — par nom, par liste de
  membres, ou par ce que la machine *est*.
- **[Un document par système d'exploitation](./guide/answers/formats.md)** — l'extension est
  le format, l'endpoint choisit entre eux.
- **[Groupes et fusion](./guide/answers/grouping.md)** — une baie partage un document ; une
  machine qui diffère ne porte que sa différence.
- **[Templating](./guide/answers/templating.md)** — `{{ serial }}` dans un groupe couvre cinq
  cents machines.
- **[Valider ce qui sera servi](./guide/answers/validating.md)** — `render` et `check`, parce
  qu'une réponse fusionnée est un document que personne n'a jamais écrit.

## L'exploiter

- **[Déploiement](./guide/operations/deployment.md)** et
  **[Synology DSM 7](./guide/operations/synology.md)**.
- **[Sécurité](./guide/operations/security.md)** — ce que les jetons protègent, et ce qu'ils
  ne protègent pas.
- **[Le store SQLite](./guide/operations/sqlite.md)** et
  **[l'API d'administration](./guide/operations/admin-api.md)** — pour un parc administré par
  outillage plutôt qu'à la main.
- **[Dépannage](./guide/operations/troubleshooting.md)** — la ligne de log est tout le
  diagnostic disponible.

Les tables exhaustives vivent dans la [Référence](./guide/reference/index.md) : chaque
[variable d'environnement](./guide/reference/configuration.md), la
[surface HTTP](./guide/reference/endpoints.md), les
[tables de formats et d'alias](./guide/reference/formats.md), et la
[ligne de commande](./guide/reference/cli.md).

## Travailler sur rescriptum

L'espace [Développement](./development/index.md) est l'autre moitié de ce site : les
[contraintes](./development/constraints.md) qui façonnent le code et pourquoi elles ne sont
pas négociables, le [cycle de vie d'une requête](./development/request-lifecycle.md), les
internes de la [sélection](./development/selection.md), des
[formats](./development/formats.md) et des [stores](./development/stores.md), comment les
[tests](./development/testing.md) sont organisés, et comment une
[release](./development/releasing.md) est faite.
