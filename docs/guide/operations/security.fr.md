---
title: Sécurité
description: Deux jetons qui se comportent délibérément différemment, ce qu'ils protègent, et ce qu'aucun des deux ne fait.
sidebar:
  label: Sécurité
  order: 3
---

# Sécurité

Les documents de réponse portent `root-password-hashed` et `root-ssh-keys`. **Qui peut les
lire peut se connecter à chaque machine que vous installez ; qui peut les écrire décide de
ces identifiants.** C'est tout le modèle de menace, et il mérite d'être dit franchement.

## L'endpoint de réponse est ouvert par défaut

Par défaut, quiconque atteint le port peut récupérer une réponse. Ce n'est pas un oubli :
**la plupart des installateurs n'ont aucun identifiant à présenter.** Un client kickstart qui
va chercher `inst.ks=http://…` n'a rien à offrir, et le refuser reviendrait à refuser
l'installation.

Le bon contrôle principal, c'est le réseau. Un VLAN de provisioning où ne se trouvent que des
machines en cours de démarrage PXE vaut plus que n'importe quel jeton.

## `RESCRIPTUM_ANSWER_TOKEN`

Proxmox *peut* présenter un identifiant quand son ISO a été préparée pour :

```console
$ proxmox-auto-install-assistant prepare-iso … --answer-auth-token 'une-longue-chaine-aleatoire'
$ export RESCRIPTUM_ANSWER_TOKEN='une-longue-chaine-aleatoire'
```

L'installateur envoie alors `Authorization: Bearer …`, et le serveur refuse tout ce qui ne
l'a pas, en comparant en temps constant.

**Les échecs ici sont journalisés mais jamais limités en débit.** Une baie entière peut se
trouver derrière une seule adresse, et l'exclure transformerait un mauvais jeton en
déploiement raté. L'[API d'administration](#le-jeton-de-lapi-dadministration), à laquelle
aucun installateur ne parle, verrouille bel et bien.

Un jeton de moins de 16 caractères est un **avertissement** au démarrage, pas une erreur —
refuser de démarrer laisserait un parc incapable de s'installer.

`GET /health` reste ouvert dans tous les cas, pour que la supervision ne s'éteigne pas.

## Le jeton de l'API d'administration

`RESCRIPTUM_ADMIN_TOKEN` est une chose différente protégeant une surface différente, et il est
traité en conséquence. L'API d'administration décide du mot de passe root et des clés SSH de
chaque machine installée ensuite, donc :

- **elle ne partage jamais le listener de l'endpoint de réponse** — elle a son propre
  `RESCRIPTUM_ADMIN_ADDR` ;
- le serveur **refuse de démarrer** sans jeton, avec un jeton de moins de 16 caractères, ou
  au-dessus du store fichiers — des erreurs, pas des avertissements ;
- une adresse qui insiste est **exclue** : cinq échecs en une minute valent un blocage,
  doublant à chaque récidive jusqu'à quinze minutes, et le blocage s'applique aussi à un
  jeton *correct* venant de cette adresse — sinon deviner jusqu'à tomber juste ne coûterait
  rien.

Générez-en un vrai. Pas un mot auquel vous avez pensé :

```console
$ openssl rand -hex 24        # ou : head -c 24 /dev/urandom | base64
```

Détails complets sur la [page de l'API d'administration](./admin-api.md#veiller-sur-le-jeton).

## Pourquoi une comparaison en temps constant

Un `==` ordinaire s'arrête dès que deux octets diffèrent, donc un mauvais jeton partageant un
préfixe plus long met mesurablement plus de temps à être rejeté. Cette différence suffit à
récupérer un jeton octet par octet — quelques milliers de requêtes plutôt qu'un nombre
impossible. Comparer tous les octets quoi qu'il arrive supprime le signal.

Sur un réseau, le timing se perd généralement dans la gigue, donc c'est une précaution. Ça
coûte cinq lignes.

## Ne mettez pas un jeton sur une ligne de commande

Tout ce qui se trouve dans les arguments d'un processus est visible par tous les autres
utilisateurs de la machine via `ps`. Cela inclut le mettre directement dans une tâche
planifiée DSM. Gardez-le dans un fichier réservé à root :

```sh
# /etc/rescriptum.env   (chmod 600, appartenant à root)
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/srv/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
```

Puis donnez-le au serveur — `EnvironmentFile=/etc/rescriptum.env` sous systemd, ou
[`RESCRIPTUM_ENV_FILE=/etc/rescriptum.env`](../reference/configuration.md#le-fichier-denvironnement)
partout ailleurs. La seconde forme fait lire le fichier par le binaire : un fichier
illisible devient donc une erreur de démarrage plutôt qu'un serveur tournant discrètement
sans le jeton que vous croyiez avoir posé. Le fichier n'est jamais découvert tout seul — il
n'y a pas de `./.env`, délibérément : ce processus tourne en root, et un fichier ramassé
dans le répertoire courant serait un moyen de donner le jeton admin à quelqu'un.

## Ce que le serveur refuse de lui-même

| | |
|---|---|
| **Traversée de chemin** | un chemin de système de fichiers n'est **jamais** construit à partir de données de requête. Seules les entrées directes du répertoire de réponses sont lues. Les identifiants arrivant à l'API d'administration n'acceptent que lettres, chiffres et `- _ . :`, parce qu'`export` les retransforme en noms de fichiers |
| **Corps surdimensionnés** | un `Content-Length` invraisemblable est refusé depuis l'en-tête, avant toute lecture ; le corps est plafonné à 1 Mo quoi qu'il arrive |
| **Clients lents** | un délai de lecture des en-têtes **et** une échéance sur la connexion entière, pour qu'un client qui promet un corps sans l'envoyer ne puisse pas garer une connexion |
| **Rafales** | au-delà de `RESCRIPTUM_MAX_CONNECTIONS` en vol, un `503` immédiat et fermeture, plutôt qu'une mise en file jusqu'à l'épuisement mémoire |
| **Entrées malformées** | un échec de parsing est une réponse d'erreur et une ligne de log, jamais un panic emportant une connexion — ou un serveur — en pleine installation |

## TLS

Le serveur parle HTTP en clair. Sur un réseau de provisioning de confiance c'est
normalement très bien, et c'est ce qui garde le binaire petit et sans dépendance.

Si vous avez besoin de TLS — certaines versions d'installateur veulent une empreinte de
certificat lors d'une récupération en HTTPS — terminez-le devant avec nginx ou Caddy et
pointez l'ISO dessus. L'endpoint de réponse se moque de ce qu'il y a en amont.

L'API d'administration est le seul endroit où cela compte par défaut : elle parle aussi HTTP
en clair, donc le jeton traverse le réseau en clair. Sur la boucle locale c'est sans objet.
Ailleurs, mettez un proxy terminant TLS devant.

## Connu et accepté

- **La limitation par adresse n'arrête pas un attaquant disposant de nombreuses adresses.**
  C'est la *longueur* du jeton d'administration qui rend la devinette sans espoir — d'où le
  plancher de 16 caractères.
- **L'endpoint de réponse n'est pas limité en débit du tout**, délibérément, pour la raison
  ci-dessus.
- **Binder l'API d'administration au-delà de la boucle locale est votre choix**, et le
  serveur le dit dans le log quand vous le faites. `127.0.0.1` plus un tunnel SSH est le
  défaut sûr.
