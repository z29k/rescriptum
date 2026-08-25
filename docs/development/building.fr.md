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
./build.sh armv7-unknown-linux-musleabihf
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
| `armv7-unknown-linux-musleabihf` | le DS416j, la raison d'être du projet | zigbuild |
| `aarch64-unknown-linux-musl` | NAS ARM récents, Raspberry Pi | zigbuild |
| `x86_64-unknown-linux-musl` | la plupart des autres hôtes Linux | zigbuild |
| `aarch64-apple-darwin` | développement local | natif |
| `x86_64-apple-darwin` | développement local | natif |

## Compilation croisée

[`cargo-zigbuild`](https://github.com/rust-cross/cargo-zigbuild) utilise Zig comme éditeur de
liens, ce qui évite une toolchain croisée complète par cible :

```bash
cargo install cargo-zigbuild
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

**Vérifiez que c'est bien statique.** Un binaire musl lié dynamiquement échoue au moment de
l'exec, sur le NAS, avec une erreur qui ne le dit pas franchement :

```console
$ file target/armv7-unknown-linux-musleabihf/release/rescriptum
ELF 32-bit LSB executable, ARM, EABI5 version 1 (SYSV), statically linked, stripped
```

La CI l'affirme à chaque push, pour exactement cette cible.

### Installer Zig sur la machine du mainteneur

Zig n'est **pas** une installation Homebrew ici : `brew install` avorte sur cette machine à
cause de taps tiers non fiables sans rapport avec Zig. Il vit dans `~/.local/zig`, avec un lien
symbolique dans `~/.local/bin/zig`. **Pour le mettre à jour, remplacez ce répertoire** —
`brew upgrade zig` ne fait rien.

Toolchain vérifiée : Rust 1.93, `cargo-zigbuild` 0.23.0, Zig 0.16.0, avec les cibles
`aarch64-apple-darwin` et `armv7-unknown-linux-musleabihf` installées.

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
| `TARGET` | `armv7-unknown-linux-musleabihf` |
| `ANSWERS` | `<répertoire-distant>/answers` |
| `PORT` | `8000` |
