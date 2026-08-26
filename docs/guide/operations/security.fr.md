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

## L'application de bureau

Sur Synology uniquement, et uniquement là — l'[application DSM](./synology.md#lapplication-de-bureau)
fait partie du paquet, pas du serveur. Son backend est un CGI que DSM sert depuis
`/webman/3rdparty/rescriptum/`, et deux choses concernant ce chemin décident de tout son
modèle de sécurité. **Les deux ont été mesurées sur une machine DSM 7.2.2 plutôt que lues
dans un guide, qui n'en mentionne aucune :**

1. **Un CGI y tourne sous le propriétaire du script.** DSM attribue l'arborescence d'un
   paquet à l'utilisateur du paquet : le backend tourne donc en tant que `rescriptum`, la
   même identité qui possède le fichier d'environnement en `0600` et le journal. C'est ce
   qui lui permet d'éditer la configuration et de lire le journal *pendant que le serveur
   est arrêté*, c'est-à-dire précisément quand un panneau de réglages sert à quelque chose.
   Ce n'est pas root, et il ne peut devenir personne : il n'a aucun droit de démarrer ou
   d'arrêter le paquet, et c'est pourquoi le redémarrage passe par l'API de DSM avec la
   session de l'administrateur. (Un script resté possédé par root, lui, **tourne bien** en
   root là-bas. Bon à savoir, et à ne jamais faire.)
2. **DSM n'authentifie pas ce chemin.** Une requête non authentifiée atteint le script et
   reçoit une réponse. DSM protège ses propres pages ; celles d'un paquet regardent le
   paquet.

Mis ensemble : les contrôles à l'intérieur du script sont la seule chose devant lui. Il en
fait donc trois, dans cet ordre, avant de toucher à quoi que ce soit.

- **Une session DSM.** Il exécute l'`authenticate.cgi` de DSM, qui affiche le nom de
  l'utilisateur connecté et n'affiche rien du tout s'il n'y a pas de session.
- **Un administrateur.** Être connecté ne suffit pas ; l'utilisateur doit appartenir à
  `administrators`. Moins que cela laisserait n'importe quel compte du NAS fixer le mot de
  passe root de chaque machine qu'il installe.
- **L'intention, pour une écriture.** Une écriture doit porter un en-tête que l'application
  envoie et qu'un formulaire d'un autre site ne peut pas : un navigateur n'envoie pas un
  en-tête inventé en cross-origin sans un préalable (*preflight*), et ce script n'y répond
  pas. Le `SynoToken` de DSM est envoyé en plus, ce qui garde l'application fonctionnelle
  avec la protection contre la falsification de requête inter-sites activée.

`check-spk.sh` vérifie que les deux premiers sont toujours dans le script, et
`lifecycle-test.sh` le pilote avec un authentificateur bouchonné pour prouver que les trois
refusent réellement. Ils ont été vus échouer : retirer le contrôle de session fait passer
quatre verts au rouge.

L'application ne reçoit jamais de jeton. `RESCRIPTUM_ANSWER_TOKEN` et
`RESCRIPTUM_ADMIN_TOKEN` lui parviennent comme *défini* ou *non défini*, et rien de plus —
la commande qu'elle appelle refuse d'afficher un identifiant, quoi qu'on lui demande.

## Connu et accepté

- **La limitation par adresse n'arrête pas un attaquant disposant de nombreuses adresses.**
  C'est la *longueur* du jeton d'administration qui rend la devinette sans espoir — d'où le
  plancher de 16 caractères.
- **L'endpoint de réponse n'est pas limité en débit du tout**, délibérément, pour la raison
  ci-dessus.
- **Binder l'API d'administration au-delà de la boucle locale est votre choix**, et le
  serveur le dit dans le log quand vous le faites. `127.0.0.1` plus un tunnel SSH est le
  défaut sûr.
