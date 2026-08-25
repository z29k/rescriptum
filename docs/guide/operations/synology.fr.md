---
title: Synology DSM 7
description: La cible d'origine — un DS416j ARMv7 avec 512 Mo et pas de Docker. Autostart, pare-feu, et remplacer une instance en cours.
sidebar:
  label: Synology DSM 7
  order: 2
---

# Synology DSM 7

Un Synology DS416j est la raison d'être de ce projet : ARMv7, 512 Mo de RAM, DSM 7, pas de
Docker. Un binaire statique sans runtime n'y est pas une préférence esthétique — c'est la
seule chose qui rentre.

DSM ne vous donne pas de systemd, donc l'autostart passe par le planificateur de tâches.

## 1. Mettre le binaire dessus

Utilisez le build **`armv7-unknown-linux-musleabihf`** de la
[page des releases](https://github.com/z29k/rescriptum/releases), ou compilez-en un
vous-même (voir [construire](../../development/building.md)).

```console
$ scp rescriptum admin@nas:/volume1/netboot/rescriptum
$ ssh admin@nas chmod +x /volume1/netboot/rescriptum
```

Si ARMv7 se comporte mal, confirmez la vraie architecture avant de supposer :

```console
$ ssh admin@nas uname -m
armv7l
```

Un DS918+ ou tout modèle x86 veut `x86_64-unknown-linux-musl` ; un DS220j et les modèles ARM
plus récents veulent `aarch64-unknown-linux-musl`.

Le build doit être **lié statiquement** — la glibc de DSM est assez ancienne pour qu'un
binaire lié dynamiquement échoue au moment de l'exec, sur le NAS, avec une erreur qui ne le
dit pas franchement :

```console
$ file rescriptum
ELF 32-bit LSB executable, ARM, EABI5 version 1 (SYSV), statically linked, stripped
```

## 2. Mettre vos réponses à côté

```console
$ ssh admin@nas mkdir -p /volume1/netboot/answers
```

`/volume1/netboot/` est l'emplacement d'un dossier partagé DSM, donc le foyer naturel du
binaire comme des réponses. Ce n'est pas une valeur par défaut : `RESCRIPTUM_ANSWERS_DIR`
vaut `/srv/answers`, qui n'existe pas sur DSM, il faut donc la définir explicitement ici. Le
fichier d'environnement ci-dessous est l'endroit le plus propre pour le faire.

## 3. Autostart

**Panneau de configuration → Planificateur de tâches → Créer → Tâche déclenchée → Script
défini par l'utilisateur**

| Champ | Valeur |
|---|---|
| Événement | **Démarrage** |
| Utilisateur | `root` |
| Commande | voir ci-dessous |

```sh
RESCRIPTUM_ANSWERS_DIR=/volume1/netboot/answers /volume1/netboot/rescriptum
```

Si vous utilisez un jeton, **ne le mettez pas dans cette case.** Tout ce qui se trouve dans
les arguments d'un processus — et, dans le cas de DSM, dans la définition de la tâche — est
lisible par tous les utilisateurs de la machine via `ps`. Mettez la configuration dans un
fichier réservé à root et nommez-le :

```sh
# /volume1/netboot/rescriptum.env   (chmod 600, appartenant à root)
RESCRIPTUM_ANSWERS_DIR=/volume1/netboot/answers
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/volume1/netboot/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
RESCRIPTUM_ANSWER_TOKEN=…
```

```sh
# l'entrée du planificateur lance ceci
RESCRIPTUM_ENV_FILE=/volume1/netboot/rescriptum.env exec /volume1/netboot/rescriptum
```

**Préférez ceci au sourcing.** L'ancienne forme —
`. /volume1/netboot/rescriptum.env && exec …` — fonctionne, et fonctionne toujours, mais
elle échoue *en silence* : que le `.` initial saute, qu'une ligne comporte une faute de
frappe, ou que les permissions soient mauvaises, et le shell ne source rien pendant que le
serveur démarre sur ses **valeurs par défaut** — répertoire de réponses par défaut, pas de
jeton admin, et pas un mot dans le log. Avec `RESCRIPTUM_ENV_FILE`, le binaire lit le
fichier lui-même et **refuse de démarrer** s'il n'y arrive pas. Il avertit aussi si le
fichier est lisible par quelqu'un d'autre que root, et nomme toute clé qu'il ne reconnaît
pas — un `RESCRIPTUM_ADMIN_TOKENN` est donc attrapé au lieu d'être ignoré discrètement.

Le détail du format est dans la
[référence de configuration](../reference/configuration.md#le-fichier-denvironnement).

Lancez la tâche une fois à la main depuis le planificateur plutôt que d'attendre un
redémarrage pour découvrir qu'elle ne marche pas.

## 4. Ouvrir le port

**Panneau de configuration → Sécurité → Pare-feu** — autorisez le TCP 8000 (ou ce que vous
avez mis dans `RESCRIPTUM_LISTEN_ADDR`) depuis votre réseau de provisioning.

Le pare-feu de DSM est de loin la raison la plus fréquente pour laquelle une machine
« ne contacte jamais le serveur ».

## 5. Vérifier

```console
$ curl http://NAS_IP:8000/health
OK
```

## Où va le log

Nulle part, par défaut : le planificateur de DSM jette la sortie d'une tâche. Nommez un
fichier dans le fichier d'environnement et le serveur y écrit lui-même, sans redirection
shell à rater :

```sh
RESCRIPTUM_LOG_FILE=/volume1/netboot/rescriptum.log
```

La ligne de log est tout le diagnostic disponible quand une installation PXE ne démarre pas,
ce n'est donc pas optionnel. Une fois le déploiement devenu routinier,
`RESCRIPTUM_LOG=problems` garde les échecs et jette les réponses réussies, la seule chose
volumineuse là-dedans. Faites tourner le fichier vous-même ; le serveur ne le fait pas.

## Remplacer une instance en cours

```console
$ ./deploy.sh admin@nas
```

Il construit pour ARMv7, [vérifie les réponses d'abord](../answers/validating.md), copie le
binaire sous un nom temporaire pour qu'un fichier à moitié copié ne soit jamais exécuté,
redémarre, et confirme que `/health` répond. Détails dans
[déploiement](./deployment.md#remplacer-une-instance-en-cours).

L'entrée du planificateur reste ce qui le démarre après un redémarrage — `deploy.sh` ne
remplace que ce qui tourne maintenant.

## Arrêt

Le planificateur de DSM envoie `SIGTERM` à l'extinction, ce que le serveur gère : il arrête
d'accepter et sort. Il n'y a de toute façon aucun état à perdre.

## À quoi s'attendre d'un DS416j

512 Mo et un cœur ARMv7, ce n'est pas beaucoup, et ce n'est pas nécessaire. Une connexion
coûte des kilo-octets plutôt qu'un thread, le listing du répertoire est mis en cache et
invalidé par mtime plutôt que parcouru à chaque requête, et un groupe sans surcharge machine
est rendu une fois au chargement puis servi comme chaîne préparée.

La seule chose à savoir : le travail sur le système de fichiers se fait sur un pool de
threads bloquants, parce que `read_dir` sur un NAS dont le disque dort n'est pas un appel
rapide, et bloquer un worker asynchrone bloquerait toutes les autres connexions qu'il pilote.
