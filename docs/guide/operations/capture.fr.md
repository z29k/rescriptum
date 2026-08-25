---
title: Capturer les requêtes
description: Enregistrer ce que les machines envoient réellement, puis le rejouer hors ligne avec render --body.
sidebar:
  label: Capturer les requêtes
  order: 4
---

# Capturer les requêtes

L'essentiel de ce que rescriptum sait des installateurs vient de leur documentation. Tant
qu'un vrai installateur ne lui a pas parlé, c'est une affirmation plutôt qu'un fait — et
quand un déploiement dérape, *« qu'est-ce que node07 a réellement envoyé ? »* est
généralement la seule question qui vaille.

```console
$ export RESCRIPTUM_CAPTURE_DIR=/var/log/rescriptum-captures
```

Désactivé sauf si défini.

## Ce qu'il écrit

Deux fichiers par requête :

```
20260824T084337Z-10.0.0.42-0000.body     le corps, verbatim
20260824T084337Z-10.0.0.42-0000.meta     qui a demandé, et ce qu'il a reçu
```

```
time: 2026-08-24T08:43:37Z
peer: 10.0.0.42:51234
request: POST /proxmox/answer
body-bytes: 1876
outcome: 200 format=toml machine=98fa9b50d810 group=rack-a
```

Le `.body` est **octet pour octet ce qui est arrivé**, donc il se rejoue sans modification.
Le nom de fichier porte l'horodatage, l'adresse du pair (assainie — les deux-points d'un pair
IPv6 n'ont rien à faire dans un nom de fichier) et un numéro de séquence, pour que deux
requêtes dans la même seconde n'entrent pas en collision.

## En rejouer une

```console
$ rescriptum render --body /var/log/rescriptum-captures/20260824T084337Z-10.0.0.42-0000.body
```

Cela résout exactement comme le serveur l'a fait, hors ligne, sans aucune machine — ce qui
rend une mauvaise réponse débogable à votre bureau plutôt que devant une baie.

C'est aussi la meilleure façon de construire des sélecteurs contre un format de corps que
vous n'avez jamais vu : capturez une vraie requête, puis itérez avec `render --body` jusqu'à
ce qu'elle se résolve comme vous le vouliez.

## Les limites, et pourquoi

- **Plafonné à 1000 captures.** Un serveur de provisioning qui remplit son propre disque est
  pire qu'un qui ne capture rien. En atteignant le plafond, il le signale une fois et arrête
  d'écrire. Le compte porte sur les captures, pas sur les fichiers, et il survit à un
  redémarrage : le serveur compte ce qui se trouve déjà dans le répertoire avant d'écrire.
- **Rien n'est jamais supprimé.** Faire tourner ou vider le répertoire vous incombe ; le
  serveur compte ce qui s'y trouve déjà au démarrage pour qu'un redémarrage ne dépasse pas le
  plafond.
- **Un échec de capture ne fait jamais échouer une requête.** Il est journalisé, et
  l'installation continue. Perdre un diagnostic ne vaut pas de perdre une installation.

## Avant d'en joindre une à un rapport de bug

Un corps capturé est un inventaire matériel : adresses MAC, numéros de série de disques, DMI.
Le `.meta` dit quelle réponse a été reçue. Ni l'un ni l'autre ne contient vos hachages de mot
de passe — mais la *réponse*, si. Nettoyez donc tout ce que vous collez à côté.

## Voir aussi

- [Dépannage](./troubleshooting.md) — lire le log, et les causes habituelles.
- [Validation](../answers/validating.md) — `render` sous ses autres formes.
