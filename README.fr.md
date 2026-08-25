<p align="center">
  <img src="https://raw.githubusercontent.com/z29k/rescriptum/main/assets/rescriptum-logo.jpg" width="150" alt="rescriptum — logo" />
</p>

<h1 align="center">rescriptum</h1>

<p align="center">
  <strong>Un serveur HTTP pour compiler et rendre les fichiers de configuration des installations automatisées d'OS.</strong>
</p>

<p align="center">
  <a href="https://github.com/z29k/rescriptum/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/z29k/rescriptum/actions/workflows/ci.yml/badge.svg" /></a>
  <a href="https://github.com/z29k/rescriptum/releases"><img alt="Release" src="https://img.shields.io/github/v/release/z29k/rescriptum?color=3da638" /></a>
  <a href="./LICENSE"><img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-3da638" /></a>
</p>

<p align="center"><a href="https://github.com/z29k/rescriptum/blob/main/README.md">English</a> · <strong>Français</strong> · <a href="https://z29k.github.io/rescriptum/fr/">📖 Documentation</a> · <a href="https://z29k.github.io/rescriptum/fr/guide/quickstart">Démarrage rapide</a> · <a href="https://z29k.github.io/rescriptum/fr/development/">Développement</a></p>

---

**Installer un parc sans écrire un fichier par machine - une seule config, et seulement les
différences.** Il répond à n'importe quel installateur automatisé : Proxmox, Debian, RHEL,
Ubuntu, Flatcar, SUSE, Windows. Il reconnaît la machine à son adresse MAC, son numéro de
série ou son inventaire matériel, empile les couches qui la concernent et renvoie le
résultat. Ce qu'une machine s'apprête à recevoir, vous pouvez le lire avant de l'allumer.

Les installateurs demandent de deux façons, et rescriptum répond aux deux, sur n'importe quel
chemin :

- **Ils POSTent ce qu'ils ont trouvé.** Proxmox VE, depuis la 8.2, envoie un inventaire JSON —
  cartes réseau et adresses MAC, disques, DMI — et attend le fichier de réponse dans le corps
  de la réponse HTTP.
- **Ils GETent avec leur identité dans la query string.** Kickstart, preseed, autoinstall
  Ubuntu, Ignition, AutoYaST : iPXE substitue la MAC ou le numéro de série dans l'URL avant
  d'aller la chercher.

```console
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers rescriptum
2026-08-24T08:43:36Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:/srv/answers workers=8 max_conn=2048 timeout=10s
2026-08-24T08:43:37Z 10.0.0.42:51234 POST /answer body=1876 200 format=toml machine=98fa9b50d810 group=rack-a bytes=431
```

- **N'importe quel installateur qui va chercher sa config.** `answer.toml` Proxmox,
  autoinstall Ubuntu, kickstart, preseed, Ignition, AutoYaST, `unattend.xml` Windows,
  scripts iPXE. L'extension est le format, l'URL décide lequel peut répondre, et les formats
  structurés fusionnent vraiment.
- **Un petit binaire statique.** Pas de runtime, pas d'interpréteur, pas de conteneur —
  aussi à l'aise sur un NAS ARM de 512 Mo que sur un hôte de datacenter encaissant une rafale
  de provisioning.
- **La configuration, ce sont des documents.** Un répertoire greppable, diffable, dans git si
  vous voulez. Ou SQLite, quand c'est de l'outillage qui gère plutôt qu'une personne.
- **La configuration se compose.** Une baie partage un fichier de groupe ; une machine qui
  diffère ne porte que sa différence.
- **Fait pour qu'on s'appuie dessus.** Asynchrone, concurrence bornée, timeouts à chaque étape,
  et des réponses que vous pouvez valider avant qu'elles ne soient servies.

## Trente secondes

```console
$ mkdir -p answers/groups
$ cat > answers/groups/rack-a.toml <<'TOML'
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

Voilà une baie **en tant que Proxmox**. Le même répertoire contient `groups/rack-a.ks` pour
les nœuds RHEL et `groups/rack-a.preseed` pour les Debian — même idée, autre extension. Un
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

→ [Démarrage rapide complet](https://z29k.github.io/rescriptum/fr/guide/quickstart) ·
[préparer les médias d'installation](https://z29k.github.io/rescriptum/fr/guide/iso)

## Ce qu'il fait

Chacun de ces points est un lien vers la
[documentation](https://z29k.github.io/rescriptum/fr/) — n'allez en profondeur que là où vous
êtes curieux.

- **[Choisit la bonne réponse](https://z29k.github.io/rescriptum/fr/guide/answers/selection)** —
  par nom de fichier, par liste de membres d'un groupe, ou par un bloc `[match]` revendiquant
  une machine pour ce qu'elle *est*. Déterministe : nommer bat matcher, plus de critères bat
  moins, les égalités se départagent sur le nom trié.
- **[Un document par système d'exploitation](https://z29k.github.io/rescriptum/fr/guide/answers/formats)** —
  `98fa9b50d810.toml` est cette machine *en tant que Proxmox*, `98fa9b50d810.preseed` le même
  matériel *en tant que Debian*. Les deux existent en même temps ; l'URL choisit.
- **[Des réponses qui se composent](https://z29k.github.io/rescriptum/fr/guide/answers/grouping)** —
  chaînes de groupes via `extends`, documents machine par-dessus. Les maps fusionnent, les
  tableaux remplacent, la machine gagne toujours.
- **[Templating](https://z29k.github.io/rescriptum/fr/guide/answers/templating)** —
  `fqdn = "node-{{ serial }}.example.com"`, rempli depuis la requête. La substitution se fait
  sur des valeurs parsées, donc c'est le sérialiseur du format qui échappe.
- **[Validation](https://z29k.github.io/rescriptum/fr/guide/answers/validating)** — `render`
  affiche ce qu'une machine recevrait, `check` rend tout et appelle le validateur de
  l'installateur là où il est dans le PATH.
- **[Un store SQLite](https://z29k.github.io/rescriptum/fr/guide/operations/sqlite)** et
  **[une API d'administration](https://z29k.github.io/rescriptum/fr/guide/operations/admin-api)** —
  pour un parc administré par outillage. Son propre listener, et une écriture qui s'annule
  plutôt que de laisser le jeu de réponses cassé.
- **[Capture des requêtes](https://z29k.github.io/rescriptum/fr/guide/operations/capture)** —
  enregistrer ce que les machines envoient réellement, le rejouer hors ligne avec
  `render --body`.
- **[Assez petit pour un NAS](https://z29k.github.io/rescriptum/fr/guide/operations/synology)** —
  builds musl statiques pour ARMv7, aarch64 et x86_64. Synology DSM 7 n'a pas de systemd, il a
  donc sa propre page ; partout ailleurs c'est une unité systemd ou un conteneur.

## Installation

Téléchargez un binaire depuis la [page des releases](https://github.com/z29k/rescriptum/releases)
— Linux `armv7`, `aarch64` et `x86_64` (musl, statique), plus macOS — vérifiez sa somme
SHA-256, et lancez-le. Il n'y a rien à installer.

```console
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers ./rescriptum
$ curl http://localhost:8000/health
OK
```

→ [Guide d'installation](https://z29k.github.io/rescriptum/fr/guide/install) ·
[Référence de configuration](https://z29k.github.io/rescriptum/fr/guide/reference/configuration)

## Organisation du dépôt

- **`src/`** — la crate. `main.rs` est un binaire mince par-dessus `lib.rs`.
- **`examples/`** — un exemple commenté et fonctionnel de chaque format supporté.
- **`docs/`** — [cette documentation](https://z29k.github.io/rescriptum/fr/), en anglais et en
  français, rendue et publiée par [notabene](https://z29k.github.io/notabene/).
- **`tests/`** — le vrai binaire sur une socket et en ligne de commande, plus la suite de
  conformité qui fait tourner chaque comportement contre les deux stores.

## Contribuer

```bash
cargo test                                                    # 308 tests
cargo clippy --all-targets --all-features -- -D warnings
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

[CONTRIBUTING.md](CONTRIBUTING.md) contient le modèle de branches et les conventions ;
l'espace [Développement](https://z29k.github.io/rescriptum/fr/development/) est le document
d'architecture honnête — les contraintes, les internes, et une
[liste de pièges](https://z29k.github.io/rescriptum/fr/development/traps) pour que personne ne
les rencontre deux fois.

## Licence

[MIT](LICENSE)
