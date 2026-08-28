---
title: Construire
description: Builds natifs, compilation croisée avec zigbuild, les cinq cibles de release, et le budget de taille.
sidebar:
  label: Construire
  order: 9
---

# Construire

```bash
./build.sh                    # cette machine, et affiche la taille
./build.sh --all              # toutes les cibles qu'une release livre
./build.sh --no-sqlite        # le plus petit binaire
./build.sh armv7-unknown-linux-gnueabihf
./build.sh --help
```

`build.sh` ajoute une cible Rust manquante pour vous et **avertit si un build musl est sorti
lié dynamiquement** — ce que DSM refuserait de lancer, au moment de l'exec sur le NAS plutôt
qu'au build sur votre portable.

`cargo build` tout court fonctionne aussi ; `build.sh` existe pour le rapport de taille et cet
avertissement.

## Les cibles de release

| Cible | Pour | Croisé |
|---|---|---|
| `armv7-unknown-linux-gnueabihf` | le DS416j, la raison d'être du projet — **glibc, pas musl**, voir plus bas | zigbuild, plancher 2.17 |
| `aarch64-unknown-linux-musl` | NAS ARM récents, Raspberry Pi | zigbuild |
| `x86_64-unknown-linux-musl` | la plupart des autres hôtes Linux | zigbuild |
| `aarch64-apple-darwin` | développement local | natif |
| `x86_64-apple-darwin` | développement local | natif |

## Compilation croisée

[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) utilise Zig comme éditeur de
liens, ce qui évite une toolchain croisée complète par cible :

```bash
cargo install cargo-zigbuild
cargo zigbuild --release --target armv7-unknown-linux-gnueabihf.2.17
```

## Pourquoi armv7 est la seule cible qui ne soit pas musl

Toutes les autres cibles sont en musl statique. ARMv7 est en glibc, et ce n'est pas une
préférence : c'est la seule façon dont la machine qui justifie ce projet exécute le binaire.

**Les noyaux ARMv7 de Synology sont des 3.10, et ils répondent `EINVAL` aux appels système
*time64* là où on attendrait `ENOSYS`.** musl 1.2 a fait passer `time_t` à 64 bits sur les
architectures 32 bits et tente d'abord `clock_gettime64` (ainsi que `clock_nanosleep` et le
futex temporisé), avec un repli sur l'appel 32 bits **conditionné à `ENOSYS`**. Sur un noyau
qui dit `EINVAL`, le repli n'arrive jamais et toute demande d'heure échoue. Mesuré sur un
DS416j en DSM 7.1, noyau 3.10.108 :

```console
$ ./probe
libc clock_gettime(CLOCK_REALTIME)  -> -1  errno=22 (Invalid argument)
syscall 263 (time32)                -> 0   ok
syscall 403 (time64)                -> -1  errno=22 (Invalid argument)
```

Le symptôme : un binaire qui répond à `--version` puis panique dès qu'il veut un horodatage
— `time.rs:131`, `Os { code: 22, kind: InvalidInput }`. Ce n'est ni un problème d'ABI ni un
noyau trop vieux pour les instructions, ce à quoi ça ressemble pourtant.

La glibc, en 32 bits, utilise les appels time32, et DSM fournit la sienne (2.20 sur
`armada38x`). Le build armv7 vise donc un **plancher glibc 2.17** — assez bas pour DSM, et
comme la glibc est rétrocompatible, le même binaire tourne aussi sur un Linux ARMv7 récent.

**Ce qu'il faut vérifier n'est donc plus qu'il est statique, mais qu'il ne réclame aucune
glibc plus récente que le plancher.** Plus récent échoue au moment de l'exec, sur le NAS,
en nommant une version de symbole et rien d'autre :

```console
$ readelf --dyn-syms target/armv7-unknown-linux-gnueabihf/release/rescriptum \
    | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1
GLIBC_2.17
```

La CI l'affirme à chaque push. Les cibles musl, elles, restent vérifiées comme statiques,
parce que pour elles c'est la promesse.

### Installer Zig sur la machine du mainteneur

Zig n'est **pas** une installation Homebrew ici : `brew install` avorte sur cette machine à
cause de taps tiers non fiables sans rapport avec Zig. Il vit dans `~/.local/zig`, avec un lien
symbolique dans `~/.local/bin/zig`. **Pour le mettre à jour, remplacez ce répertoire** —
`brew upgrade zig` ne fait rien.

Toolchain vérifiée : Rust 1.93, `cargo-zigbuild` 0.23.0, Zig 0.16.0, avec les cibles
`aarch64-apple-darwin` et `armv7-unknown-linux-gnueabihf` installées.

## Le profil release

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
```

`panic = "abort"` est **délibérément absent** — voir
[contraintes](./constraints.md#ne-jamais-paniquer-sur-une-entrée-malformée). Coût mesuré du
maintien du déroulement sur ARMv7 : +2416 octets, +0,8 %.

## Taille

| Build | ARMv7 |
|---|---|
| par défaut | 2 103 456 octets |
| `--no-default-features` (ni SQLite, ni API d'administration) | 944 928 octets |

L'essentiel de la différence est SQLite embarqué, compilé depuis les sources. La CI construit
`--release --no-default-features` à chaque push pour que le petit build ne pourrisse pas sans
qu'on le remarque.

## Features

| Feature | Défaut | Apporte |
|---|---|---|
| `sqlite` | activée | le store SQLite et l'API d'administration |

```bash
cargo build --no-default-features          # le plus petit
cargo test --all-features                  # ce que lance la CI
```

## Le paquet Synology

Un `.spk` est un **format de release**, pas un build : le binaire est fini avant que
l'empaquetage commence, il n'y a pas de build spécifique à DSM, et rien dans `src/` ne sait
que Synology existe.

```bash
./build.sh --spk x86_64-unknown-linux-musl   # compiler, puis emballer
packaging/dsm/make-spk.sh armv7              # emballer un build qui existe déjà
packaging/dsm/check-spk.sh                   # contrôle structurel sur dist/*.spk
```

**Le paquet embarque les chargeurs : construisez-les d'abord, sinon il ne passe pas son
propre contrôle.** `make-spk.sh` les prend dans `packaging/ipxe/out` (remplaçable par
`RESCRIPTUM_LOADERS`), et `check-spk.sh` refuse un paquet qui n'en a pas — un serveur TFTP
sans rien à distribuer ne démarre personne. Construire iPXE demande une chaîne C Linux, ce
qui sur un Mac veut dire un conteneur :

```bash
docker run --rm --platform linux/amd64 -v "$PWD:/w" -w /w debian:bookworm-slim sh -c '
  apt-get update -qq &&
  apt-get install -y --no-install-recommends build-essential liblzma-dev mtools \
    xorriso isolinux gcc-aarch64-linux-gnu git ca-certificates perl &&
  packaging/ipxe/build.sh --out /w/packaging/ipxe/out'
```

Une fois, pas par paquet : les chargeurs sont les mêmes octets dans le `.spk` de chaque
ABI, puisqu'ils tournent sur les machines *démarrées*, pas sur le NAS. `packaging/ipxe/out`
est gitignoré — **jamais de binaires dans git**.

| ABI | `arch` dans `INFO` | Depuis |
|---|---|---|
| `x86_64` | `x86_64` — le nom de *famille*, donc toutes les plateformes Intel | `x86_64-unknown-linux-musl` |
| `armv7` | `armada38x` — le raccourci de famille n'atteint pas les plateformes Marvell | `armv7-unknown-linux-gnueabihf` |
| `aarch64` | `armv8` | `aarch64-unknown-linux-musl`, une fois le binaire lancé sur l'une d'elles |

La règle pour élargir : **revendiquer un ABI une fois le binaire lancé sur son membre au
noyau le plus ancien**, jamais parce qu'une plateforme est plausible.

`make-spk.sh` est déterministe — mtimes fixes, propriété `0:0`, `ustar`, `gzip -n`, liste de
fichiers pré-triée — donc les mêmes entrées donnent un `.spk` identique octet pour octet, ce
qui est ce qui donne du sens à la somme publiée.

`check-spk.sh` tourne dans la CI à chaque push. Il vérifie que l'archive externe est un tar
*non compressé*, que `INFO` a ses six champs obligatoires et une version tout en segments
numériques, que les icônes font exactement 64×64 et 256×256, que les scripts de cycle de vie
s'analysent et sont exécutables, et que **le `--version` du binaire empaqueté correspond à
`INFO`** — le build x86_64 tourne sur le runner, donc cette dernière assertion est réelle et
non une relecture de la même chaîne.

`lifecycle-test.sh` déroule ensuite les scripts du paquet contre un faux arbre
`/var/packages` — installation, démarrage, `/health`, les codes de sortie, une mise à jour
par-dessus une configuration éditée à la main, une désinstallation par-dessus un canary dans
le partage — et tourne lui aussi à chaque push.

```bash
packaging/dsm/lifecycle-test.sh
```

Ce que rien de tout cela ne peut prouver, c'est que DSM acceptera le paquet ; seule une
installation le peut. C'est le banc d'essai de
[`packaging/dsm/vm/`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/vm/README.md) :
un lanceur QEMU, et un script qui joue les vérifications sur la machine — la VM pendant
qu'on itère, le DS416j pour le verdict. Voir
[tests](./testing.md#le-paquet-aussi-est-testé-à-trois-endroits).

## Déployer un build

```bash
./deploy.sh admin@nas
./deploy.sh admin@nas /volume1/netboot
```

Construit, **vérifie les réponses et refuse d'expédier si elles ne reviennent pas propres**,
copie sous un nom temporaire, redémarre, et confirme `/health`. Voir
[déploiement](../guide/operations/deployment.md#remplacer-une-instance-en-cours).

| Environnement | Défaut |
|---|---|
| `TARGET` | `armv7-unknown-linux-gnueabihf` |
| `ANSWERS` | `<répertoire-distant>/answers` |
| `PORT` | `8000` |
