---
title: Déploiement
description: Une unité systemd, un fichier d'environnement, et comment deploy.sh remplace une instance en cours sans expédier un jeu de réponses cassé.
sidebar:
  label: Déploiement
  order: 1
---

# Déploiement

Le binaire se suffit à lui-même : copiez-le quelque part, donnez-lui un répertoire de
réponses, et lancez-le. Tout ce qui suit sert à le faire de façon reproductible.

Pour Synology DSM 7 — qui n'a pas de systemd — voir [sa page dédiée](./synology.md).

## Un fichier d'environnement

Gardez la configuration dans un fichier lisible par root seul, plutôt que dans une unité ou
sur une ligne de commande. **Tout ce qui est sur une ligne de commande est visible par tous
les utilisateurs de la machine via `ps`**, ce qui compte dès qu'un jeton entre en jeu :

```sh
# /etc/rescriptum.env   (chmod 600, appartenant à root)
RESCRIPTUM_ANSWERS_DIR=/srv/answers
RESCRIPTUM_LISTEN_ADDR=0.0.0.0:8000
RESCRIPTUM_TIMEOUT_SECS=10
# RESCRIPTUM_ANSWER_TOKEN=…
```

Sous systemd, l'`EnvironmentFile=` ci-dessous le lit et vous n'avez besoin de rien d'autre.
Ailleurs — et sur [DSM 7](./synology.md), qui n'a pas de systemd — pointez
[`RESCRIPTUM_ENV_FILE`](../reference/configuration.md#le-fichier-denvironnement) sur le même
fichier et le binaire le lit lui-même, en refusant de démarrer s'il n'y arrive pas.

## Une unité systemd

```ini
# /etc/systemd/system/rescriptum.service
[Unit]
Description=rescriptum — per-machine answer files for unattended installs
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/local/bin/rescriptum
EnvironmentFile=/etc/rescriptum.env
Restart=on-failure
RestartSec=2

# Il doit lire un répertoire et binder un port. Rien d'autre.
DynamicUser=yes
ReadOnlyPaths=/srv/answers
NoNewPrivileges=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectSystem=strict
ProtectHome=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_INET AF_INET6
SystemCallFilter=@system-service

[Install]
WantedBy=multi-user.target
```

```console
$ sudo systemctl enable --now rescriptum
$ curl -s http://localhost:8000/health
OK
```

Ajustez selon ce que vous activez réellement :

- **Store SQLite** — la base doit être accessible en écriture, donc `ReadWritePaths=/srv` et
  retirez `ReadOnlyPaths`.
- **Capture des requêtes** — `ReadWritePaths=` sur le répertoire de capture.
- **Un port sous 1024** — ajoutez `AmbientCapabilities=CAP_NET_BIND_SERVICE`.

Les logs partent sur stderr, donc `journalctl -u rescriptum -f` est la vue en direct.

## En conteneur

Il n'y a rien à installer, donc l'image est le binaire :

```dockerfile
FROM scratch
COPY rescriptum /rescriptum
ENV RESCRIPTUM_ANSWERS_DIR=/answers RESCRIPTUM_LISTEN_ADDR=0.0.0.0:8000
EXPOSE 8000
ENTRYPOINT ["/rescriptum"]
```

Utilisez le build musl de la bonne architecture — il est lié statiquement, ce qui est ce qui
fait fonctionner `FROM scratch`. Montez le répertoire de réponses en lecture seule.

## Le dimensionner

Les valeurs par défaut sont déjà bonnes aux deux extrémités de la gamme pour laquelle il a
été conçu.

| Réglage | Défaut | Le changer quand |
|---|---|---|
| `RESCRIPTUM_WORKERS` | nombre de CPU | vous partagez une petite machine et voulez plafonner les threads |
| `RESCRIPTUM_MAX_CONNECTIONS` | 2048 | vous voyez des `503` pendant une rafale — ou voulez délester plus tôt |
| `RESCRIPTUM_TIMEOUT_SECS` | 10 | les clients sont sur un lien lent, ou vous voulez couper le slowloris plus tôt |

`MAX_CONNECTIONS` n'est pas une limite de débit. Au-delà du plafond, le serveur écrit un
`503` immédiat et ferme plutôt que de mettre en file — un client à qui on dit de réessayer
s'en sort mieux qu'un client garé dans une file qui ne se videra pas.

Un déploiement de 2 000 machines se termine en moins de deux secondes au débit mesuré, donc
le dimensionnement est rarement le problème intéressant. Le
[dépannage](./troubleshooting.md) l'est généralement.

## Remplacer une instance en cours

```console
$ ./deploy.sh admin@nas
$ ./deploy.sh admin@nas /volume1/netboot        # un autre répertoire distant
```

Ce qu'il fait, dans l'ordre :

1. **Construit** pour la cible (`TARGET`, par défaut `armv7-unknown-linux-musleabihf`).
2. **Vérifie les réponses locales** avec `rescriptum check` et refuse de continuer si quoi
   que ce soit échoue — expédier un jeu de réponses cassé est pire que ne pas déployer.
3. **Copie le binaire sous un nom temporaire**, puis le renomme en place. Remplacer un
   binaire en cours d'exécution sur place est la façon dont un fichier à moitié copié se
   fait exécuter.
4. **Arrête l'instance en cours, démarre la nouvelle** en détaché, et confirme qu'elle est
   restée en vie.
5. **Confirme que `/health` répond** par le réseau, pour qu'un problème de pare-feu soit
   signalé comme tel plutôt que comme un silence mystérieux.

| Environnement | Défaut |
|---|---|
| `TARGET` | `armv7-unknown-linux-musleabihf` |
| `ANSWERS` | `<répertoire-distant>/answers` |
| `PORT` | `8000` |

Il remplace ce qui tourne ; il n'installe pas l'autostart. Sur DSM c'est une
[entrée du planificateur de tâches](./synology.md#3-autostart) ; avec systemd c'est
`systemctl enable`.

## Mettre à jour

Les réponses sont des données, pas de l'état : rien n'est migré, et un nouveau binaire lit le
même répertoire. Remplacez-le et redémarrez.

L'exception est le store SQLite, qui porte une version de schéma. Il n'y en a qu'une pour
l'instant, donc rien à migrer ; ce que la version apporte, c'est l'autre sens : un binaire
**plus ancien** refuse d'ouvrir une base écrite par un plus récent plutôt que de la deviner.
Voir [le store SQLite](./sqlite.md).
