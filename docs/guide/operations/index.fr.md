---
title: L'exploiter
description: Déploiement, sécurité, stockage, et lire le log quand un déploiement dérape.
sidebar:
  label: L'exploiter
  order: 5
  indexLabel: Vue d'ensemble
---

# L'exploiter

rescriptum est un processus, configuré entièrement par l'environnement, qui n'écrit rien en
dehors du store que vous lui indiquez. Bien l'exploiter consiste surtout à décider ce qu'il a
le droit de servir, et à qui.

- **[Déploiement](./deployment.md)** — une unité systemd, un conteneur, ou rien du tout.
- **[Synology DSM 7](./synology.md)** — la cible d'origine : une installation par Package
  Center qui crée le partage, enregistre le port et démarre au boot.
- **[Sécurité](./security.md)** — les deux jetons, pourquoi ils se comportent différemment, et
  ce qu'aucun des deux ne protège.
- **[Capturer les requêtes](./capture.md)** — enregistrer ce que les machines envoient
  réellement, et le rejouer hors ligne.
- **[Le store SQLite](./sqlite.md)** — pour un parc administré par outillage plutôt qu'à la
  main.
- **[L'API d'administration](./admin-api.md)** — gérer les réponses en HTTP, sur son propre
  listener, avec une écriture qui ne peut pas casser le parc.
- **[Dépannage](./troubleshooting.md)** — la ligne de log est tout le diagnostic disponible.
- [Servir les médias de démarrage](./media.md) — le noyau, l'initrd et l'image de l'installeur, depuis le même serveur.

## La forme d'un déploiement

| | |
|---|---|
| **Un processus** | pas d'arbre de supervision, pas de workers à dimensionner, pas de sidecar |
| **Un port** par défaut | plus un second, seulement si vous activez l'API d'administration |
| **Aucune écriture** | en dehors du répertoire de réponses ou de la base, et aucune du tout à moins d'activer l'API d'administration ou la capture |
| **Aucun état** | entre les requêtes. Un redémarrage ne perd rien |
| **Arrêt propre** | sur SIGTERM (ce qu'envoie le planificateur DSM) et Ctrl-C |

La configuration se fait par [variables d'environnement](../reference/configuration.md)
uniquement. Une valeur numérique nulle ou impossible à parser retombe sur sa valeur par
défaut plutôt que de démarrer un serveur qui accepte des connexions sans jamais répondre.

## Ce dont il a besoin du réseau

Que l'installateur puisse l'atteindre, et c'est tout. Il n'ouvre aucune connexion sortante,
n'a besoin d'aucun DNS, et se moque d'être derrière un NAT.

Le HTTP en clair est le choix normal sur un réseau de provisioning. Si vous avez besoin de
TLS — certaines versions d'installateur demandent une empreinte de certificat — terminez-le
devant avec nginx ou Caddy et pointez l'ISO dessus. Voir
[Sécurité](./security.md#tls).
