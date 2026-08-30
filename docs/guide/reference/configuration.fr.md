---
title: Configuration
description: Chaque variable d'environnement, sa valeur par défaut, les deux formats de fichier qui peuvent les fournir, et ce qui arrive quand on se trompe.
sidebar:
  label: Configuration
  order: 1
---

# Configuration

Des variables d'environnement — et, en option, un fichier d'où les lire, dans l'une de deux
formes. Il n'y a pas de ligne de commande à se tromper, et les variables *sont* toute la
configuration : les deux formats de fichier règlent exactement les mêmes choses sous
exactement les mêmes règles, donc rien de ce qu'on écrit dans un fichier ne peut signifier
quelque chose que l'environnement ne dirait pas.

## Les variables

| Variable | Défaut | Signification |
|---|---|---|
| `RESCRIPTUM_CONFIG` | non défini | Lire les valeurs par défaut depuis ce fichier **TOML** — voir [plus bas](#le-fichier-toml) |
| `RESCRIPTUM_ENV_FILE` | non défini | Lire les valeurs par défaut depuis ce fichier `CLÉ=valeur` — voir [plus bas](#le-fichier-denvironnement) |
| `RESCRIPTUM_STORE` | `files` | `files` (un répertoire) ou `sqlite` (une base) |
| `RESCRIPTUM_ANSWERS_DIR` | `/srv/answers` | Répertoire des documents de réponse |
| `RESCRIPTUM_DB_PATH` | `/srv/answers.db` | Chemin de la base, quand `RESCRIPTUM_STORE=sqlite` |
| `RESCRIPTUM_LISTEN_ADDR` | `0.0.0.0:8000` | Adresse d'écoute. `:0` choisit un port libre, et celui qui est bindé est affiché |
| `RESCRIPTUM_WORKERS` | nombre de CPU | Threads du runtime asynchrone. **Pas** une limite de concurrence |
| `RESCRIPTUM_MAX_CONNECTIONS` | `2048` | Connexions en vol avant délestage en `503` |
| `RESCRIPTUM_TIMEOUT_SECS` | `10` | Délai de lecture des en-têtes **et** échéance de la connexion entière |
| `RESCRIPTUM_ANSWER_TOKEN` | non défini | Jeton exigé par l'endpoint de réponse. Non défini = ouvert |
| `RESCRIPTUM_ADMIN_ADDR` | non défini | Listener de l'API d'administration. Non défini = API désactivée |
| `RESCRIPTUM_ADMIN_TOKEN` | non défini | Jeton d'administration, 16+ caractères. Obligatoire avec `RESCRIPTUM_ADMIN_ADDR` |
| `RESCRIPTUM_CAPTURE_DIR` | non défini | Enregistre les corps de requête ici. Non défini = pas de capture |
| `RESCRIPTUM_LOG` | `all` | `all`, `problems` ou `off` — voir [plus bas](#journalisation) |
| `RESCRIPTUM_LOG_FILE` | non défini | Un fichier où ajouter, ou `stdout` / `stderr`. Non défini = stderr |
| `RESCRIPTUM_MEDIA_DIR` | non défini | Images d'installation. **Non défini = pas de média et pas de listener média** |
| `RESCRIPTUM_MEDIA_ADDR` | `0.0.0.0:8001` | Le listener média, quand un répertoire de médias existe |
| `RESCRIPTUM_MEDIA_TIMEOUT_SECS` | `600` | Échéance du transfert entier. Volontairement pas les 10 s du point de réponse |
| `RESCRIPTUM_MEDIA_MAX_CONNECTIONS` | `16` | Transferts simultanés. Bas exprès : chacun retient son jeton des minutes durant |
| `RESCRIPTUM_PUBLIC_HOST` | déduit | L'hôte que nomment les URL générées. **Un hôte, jamais une URL** |
| `RESCRIPTUM_BOOT_ALLOW` | non défini | CIDR clients autorisés à récupérer les médias. Non défini = quiconque atteint le port |
| `RESCRIPTUM_BOOT_DIR` | non défini | Chargeurs et menus, distribués en TFTP. **Non défini = pas de TFTP du tout** |
| `RESCRIPTUM_TFTP_ADDR` | `0.0.0.0:69` | Le listener TFTP, ou **`off`** pour aucun. Le port 69 est privilégié ; voir `RESCRIPTUM_USER` |
| `RESCRIPTUM_TFTP_PORT_RANGE` | non défini | Les ports depuis lesquels un transfert répond, en `premier-dernier`. **Un transfert TFTP quitte le port 69 aussitôt** — le serveur répond depuis un port neuf et le client acquitte vers celui-là — donc un pare-feu n'autorisant que 69 jette l'acquittement, et la machine semble s'être désintéressée. Épinglez la plage pour pouvoir l'ouvrir. Non définie, le noyau choisit |
| `RESCRIPTUM_TFTP_BLKSIZE` | `1468` | Le plus grand bloc TFTP accepté. 1468 remplit **exactement** un chemin de 1500 octets — 1468 de charge, 4 TFTP, 8 UDP, 20 IP — donc un tag VLAN ou un tunnel rend la trame trop grande et une ROM PXE s'arrête en général sans rien dire. À baisser (1400, ou 512) quand un démarrage cale au premier bloc |
| `RESCRIPTUM_BOOT_TIMEOUT_SECS` | `15` | Secondes avant que le menu ne retombe sur le disque local |
| `RESCRIPTUM_BOOT_UNCLAIMED` | `menu` | Ce que reçoit une machine qu'aucune réponse ne revendique. `local` la rend à son firmware, ce qui inverse le sens d'un fichier de réponse : présent veut dire *installe celle-ci* plutôt que *laisse celle-ci tranquille* |
| `RESCRIPTUM_INSTALLED_TOKEN` | non défini | Le jeton du `[post-installation-webhook]` de Proxmox. Défini, `POST /installed` existe et retire la revendication d'installation d'une machine quand elle signale sa réussite. **Non défini, il n'y a pas d'endpoint** |
| `RESCRIPTUM_BOOT_LOGO` | intégré | Un PNG à afficher derrière le menu |
| `RESCRIPTUM_BOOT_TITLE` | intégré | La barre de titre du menu |
| `RESCRIPTUM_USER` / `_GROUP` | non défini | Basculer dessus **après** avoir lié. L'ordre inverse échoue au déploiement |

`/srv` est l'endroit où la norme de hiérarchie des fichiers range les données servies par le
système, ce qu'est précisément un répertoire de réponses. Les deux valeurs par défaut y vivent,
pour qu'un `rescriptum` lancé sans rien fasse quelque chose de plausible sur n'importe quel
hôte Linux. Rien ne crée le répertoire pour vous, et le serveur le signale au démarrage s'il
manque.

## Journalisation

Une ligne par événement, sur stderr par défaut. Deux réglages, parce que les deux questions
sont différentes.

**Quoi** — `RESCRIPTUM_LOG` :

| Valeur | Garde |
|---|---|
| `all` (défaut) | chaque requête, plus le démarrage, les avertissements et les erreurs |
| `problems` | démarrage, avertissements, erreurs, et seulement les requêtes qui **n'ont pas** abouti |
| `off` / `none` | rien du tout |

Une réponse réussie fait une ligne, et à treize mille requêtes par seconde c'est la seule
chose ici qui ait du volume. `problems` est ce que vous voulez quand un déploiement devient
routinier et que le disque, lui, ne l'est pas. Tout le reste est peu volumineux et
diagnostique, donc conservé dans les deux cas.

Une requête qui n'a jamais atteint de statut — une connexion expirée en plein corps —
compte comme un problème. Une valeur non reconnue retombe sur `all` avec un avertissement :
une faute de frappe ne doit pas être la raison pour laquelle personne ne voit pourquoi un
déploiement a échoué. Le niveau est nommé dans la ligne de démarrage (`log=problems`), donc
un log vide s'explique lui-même.

**Où** — `RESCRIPTUM_LOG_FILE` :

| Valeur | Va vers |
|---|---|
| non définie, ou `stderr` | stderr, ce que lit un superviseur |
| `stdout` | stdout |
| toute autre valeur | ce fichier, en ajout ; les répertoires parents sont créés |

Un fichier impossible à ouvrir est une **erreur de démarrage**, pas un repli sur stderr —
ce serait une surprise silencieuse découverte bien plus tard. Une écriture qui échoue une
fois le serveur lancé est abandonnée : un serveur de provisioning qui mourrait parce que
son disque de logs est plein ferait échouer toutes les installations en cours pour signaler
qu'il ne peut pas signaler quelque chose.

La rotation vous incombe. Sous systemd il n'y a rien à faire, le log part dans le journal ;
avec un fichier, pointez `logrotate` dessus avec `copytruncate`.

## Le fichier TOML

`RESCRIPTUM_CONFIG` nomme un fichier en TOML qui règle les mêmes variables, dans une forme
faite pour être lue. C'est celui vers lequel se tourner quand **une personne édite le
fichier à la main** — sur un NAS, dans File Station ou via SMB — c'est-à-dire exactement là
où `RESCRIPTUM_ANSWERS_DIR=…` sur chaque ligne se lit mal, et où le mot « environnement »
envoie chercher un shell qui n'existe pas.

```toml
# /etc/rescriptum.toml   (chmod 600, appartenant à root)
answers_dir = "/srv/answers"
listen_addr = "0.0.0.0:8000"
log         = "problems"          # all | problems | off

[store]
kind    = "sqlite"
db_path = "/srv/answers.db"

[server]
workers         = 2
max_connections = 2048
timeout_secs    = 10

[admin]
addr  = "127.0.0.1:8001"
token = "…"

[answer]
token       = "…"
capture_dir = "/var/lib/rescriptum/captures"
```

```console
$ RESCRIPTUM_CONFIG=/etc/rescriptum.toml rescriptum
2026-08-29T12:42:02Z - reading configuration defaults from /etc/rescriptum.toml (8 set)
```

Toutes les règles du fichier d'environnement valent ici aussi : **jamais découvert,
seulement nommé** (il n'y a pas de `./rescriptum.toml`) ; **l'environnement réel gagne** ;
et **un fichier demandé et illisible est une erreur de démarrage**, jamais un
avertissement.

**Placez-le hors du répertoire de réponses.** Tout `.toml` servable à la racine de ce
répertoire est un document réponse, et ce format partage l'extension — un fichier de
configuration déposé là est signalé par `check` comme une réponse mal placée, et `migrate`
propose de le déplacer. `/etc` est le foyer évident ; sur une installation empaquetée, le
paquet en choisit un.

### Les noms

Le préfixe disparaît et les tables font le regroupement. Rien d'autre ne change : chaque
ligne ci-dessous est la variable du même nom, et `rescriptum config` affiche les deux
orthographes.

| Dans le fichier | Variable |
|---|---|
| `answers_dir` | `RESCRIPTUM_ANSWERS_DIR` |
| `listen_addr` | `RESCRIPTUM_LISTEN_ADDR` |
| `log`, `log_file` | `RESCRIPTUM_LOG`, `RESCRIPTUM_LOG_FILE` |
| `public_host` | `RESCRIPTUM_PUBLIC_HOST` |
| `user`, `group` | `RESCRIPTUM_USER`, `RESCRIPTUM_GROUP` |
| `store.kind`, `store.db_path` | `RESCRIPTUM_STORE`, `RESCRIPTUM_DB_PATH` |
| `server.workers`, `server.max_connections`, `server.timeout_secs` | `RESCRIPTUM_WORKERS`, `RESCRIPTUM_MAX_CONNECTIONS`, `RESCRIPTUM_TIMEOUT_SECS` |
| `admin.addr`, `admin.token` | `RESCRIPTUM_ADMIN_ADDR`, `RESCRIPTUM_ADMIN_TOKEN` |
| `answer.token`, `answer.capture_dir` | `RESCRIPTUM_ANSWER_TOKEN`, `RESCRIPTUM_CAPTURE_DIR` |
| `media.dir`, `media.addr`, `media.timeout_secs`, `media.max_connections` | les quatre `RESCRIPTUM_MEDIA_*` |
| `boot.dir`, `boot.allow`, `boot.unclaimed`, `boot.timeout_secs`, `boot.logo`, `boot.title` | les six `RESCRIPTUM_BOOT_*` |
| `tftp.addr`, `tftp.port_range`, `tftp.blksize` | les trois `RESCRIPTUM_TFTP_*` |
| `installed.token` | `RESCRIPTUM_INSTALLED_TOKEN` |

### Le format

| | |
|---|---|
| N'importe quel scalaire TOML | un nombre peut s'écrire en nombre (`workers = 2`) ou en chaîne ; les deux arrivent au serveur comme le même réglage |
| Un commentaire `#` | n'importe où, y compris en fin de ligne — contrairement au fichier d'environnement, qui n'a pas d'échappements et ne peut donc pas en avoir |
| Une valeur avec un `#`, un guillemet ou une espace | sans problème, échappée comme TOML échappe, et relue à l'identique |
| `""` | **non défini** — la même règle qu'une variable exportée mais vide, et c'est ce qui permet à `config unset` de vider une ligne au lieu de supprimer le paragraphe qui la documente |
| La même clé deux fois | refusée par TOML lui-même, donc le fichier ne se charge pas |
| Une clé que ce programme ne lit pas | un avertissement la nommant : `admin.tokenn` est attrapé au lieu d'être ignoré |
| Une liste ou une table là où une valeur est attendue | une **erreur de démarrage** : contrairement à une faute de frappe, elle visait un réglage réel, et servir la valeur par défaut alors que le fichier dit le contraire serait silencieux |
| Un fichier lisible par d'autres | un avertissement avec son mode, parce qu'il peut contenir `admin.token` |

Les avertissements nomment les clés et les chemins, jamais les valeurs.

### Les deux fichiers à la fois

Nommer les deux est une transition plutôt qu'un état stable : rien n'est refusé et l'ordre
est annoncé au démarrage — **l'environnement bat le fichier TOML, qui bat le fichier
d'environnement.** `rescriptum config` montre lequel des trois a mis chaque valeur en
vigueur, et `config set` écrit dans le fichier TOML — celui que le serveur lit en premier,
pour qu'une écriture ne puisse pas être une modification qui ne change rien en silence.

## Le fichier d'environnement

`RESCRIPTUM_ENV_FILE` nomme un fichier contenant les mêmes variables. Il existe pour les
déploiements qui n'ont nulle part où mettre un jeton — au premier chef **Synology DSM 7,
qui n'a pas de systemd**. Sous systemd, `EnvironmentFile=` fait déjà cela et vous n'en avez
pas besoin.

```sh
# /etc/rescriptum.env   (chmod 600, appartenant à root)
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/srv/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
```

```console
$ RESCRIPTUM_ENV_FILE=/etc/rescriptum.env rescriptum
2026-08-24T12:42:02Z - reading configuration defaults from /etc/rescriptum.env (4 set)
```

**Il n'est jamais découvert, seulement nommé.** Il n'y a pas de `./.env`. Ce binaire tourne
en root : s'il ramassait un fichier dans le répertoire d'où il a été lancé, quiconque peut
y écrire posséderait `RESCRIPTUM_ADMIN_TOKEN` — et avec lui le mot de passe root de chaque
machine installée ensuite.

**L'environnement réel gagne.** Le fichier fournit des valeurs par défaut, donc ce qui est
exporté délibérément au lancement n'est jamais écrasé en douce. Une variable exportée mais
vide compte comme non définie, donc le fichier s'applique quand même.

**Un fichier demandé et illisible est une erreur de démarrage**, pas un avertissement.
C'est tout l'intérêt : l'échec qu'il remplace est un serveur qui démarre sur ses valeurs
par défaut — mauvais répertoire de réponses, pas de jeton admin — sans un mot dans le log.

### Le format

| | |
|---|---|
| `CLÉ=valeur`, une par ligne | un `export` en tête est accepté, pour que le même fichier puisse aussi être `source`é |
| `#` en **début de ligne** | un commentaire |
| `#` ailleurs | **fait partie de la valeur.** Pas de commentaires en fin de ligne : tronquer un jeton sur un `#` qu'il contient légitimement serait silencieux, alors qu'un commentaire atterrissant dans une valeur est bruyant |
| `"guillemets"` ou `'guillemets'` | les guillemets sont retirés et les espaces internes conservés ; une valeur sans guillemets est trimée |
| `$HOME`, `${x}` | **non développés.** Ce n'est pas un shell — pas de substitution, pas de lignes de continuation |
| la même clé deux fois | une erreur de démarrage, plutôt qu'une supposition sur celle qui était voulue |
| une clé que ce programme ne lit pas | un avertissement nommant la clé — `RESCRIPTUM_ADMIN_TOKENN` est donc attrapé au lieu d'être ignoré |
| un fichier lisible par d'autres | un avertissement avec son mode, parce qu'il peut contenir le jeton admin |

Les avertissements nomment les clés et les chemins, jamais les valeurs.

## Le lire et le modifier

`rescriptum config` affiche chaque variable, sa valeur, et **qui des fichiers ou de
l'environnement l'y a mise** — la distinction qui compte, puisque les fichiers fournissent
des valeurs par défaut et que l'environnement réel l'emporte. `config set` modifie le
fichier comme on voudrait qu'il le soit, dans l'un ou l'autre format : commentaires
conservés, réglage commenté décommenté sur place plutôt que dupliqué (fichier
d'environnement) ou valeur remplacée là où elle est (TOML), et refus avant toute écriture
d'une modification qui laisserait
un serveur incapable de démarrer. C'est documenté dans la
[référence de la ligne de commande](./cli.md#config), et c'est ce que
l'[application DSM](../operations/synology.md#lapplication-de-bureau) pilote dessous.

## Valeurs invalides

| Cas | Ce qui se passe |
|---|---|
| Exportée mais vide (`RESCRIPTUM_LISTEN_ADDR=`) | traitée comme **non définie** — une valeur vide est une erreur, pas une instruction |
| Uniquement des espaces | pareil, et les valeurs sont trimées |
| Un nombre nul ou impossible à parser | retombe sur la **valeur par défaut**, plutôt que de démarrer un serveur qui accepte des connexions sans jamais répondre |
| `RESCRIPTUM_STORE` avec toute autre valeur | un avertissement, et `files` est utilisé |
| `RESCRIPTUM_ENV_FILE` ou `RESCRIPTUM_CONFIG` nommant un fichier absent, illisible ou malformé | une **erreur** de démarrage |
| Un réglage TOML recevant une liste ou une table | une **erreur** de démarrage, contrairement à une clé mal orthographiée, qui avertit |
| `RESCRIPTUM_STORE=sqlite` sur un binaire construit sans la feature | une **erreur** au démarrage |

## Erreurs de démarrage

Celles-ci arrêtent le serveur au lieu d'avertir, parce que démarrer quand même serait pire :

| Condition | Pourquoi c'est fatal |
|---|---|
| `RESCRIPTUM_ADMIN_ADDR` défini avec `RESCRIPTUM_STORE` autre que `sqlite` | deux façons de changer la même configuration, en concurrence |
| `RESCRIPTUM_ADMIN_ADDR` défini sans `RESCRIPTUM_ADMIN_TOKEN` | une API ouverte qui réécrit les identifiants root |
| `RESCRIPTUM_ADMIN_TOKEN` de moins de 16 caractères | assez court pour être deviné |
| L'adresse d'écoute ne peut pas être bindée | rien à faire |
| Le store ne peut pas être ouvert | rien à servir |
| `RESCRIPTUM_MEDIA_ADDR` défini sans `RESCRIPTUM_MEDIA_DIR` | un listener sans rien à servir |
| `RESCRIPTUM_MEDIA_ADDR` égal à l'adresse de réponse ou d'administration | le second bind perd, et lequel dépend de l'ordre de démarrage |
| `RESCRIPTUM_PUBLIC_HOST` portant un schéma, un port ou un chemin | il est écrit dans les URL de deux listeners ; un port dans la valeur épingle chaque script généré sur l'un d'eux |
| `RESCRIPTUM_TFTP_ADDR` défini sans `RESCRIPTUM_BOOT_DIR` | un listener sans chargeur à distribuer |
| Le répertoire de démarrage ne peut pas être résolu | chaque contrôle de chemin s'y compare |
| `RESCRIPTUM_USER` nomme un compte inexistant | rien à devenir |

## Avertissements de démarrage

Ceux-ci sont affichés et le serveur continue :

| Condition | Ligne |
|---|---|
| Répertoire de réponses absent | `warning: … does not exist yet — every request will 404 until it does` |
| Le chemin existe mais n'est pas un répertoire | `warning: … is not a directory — every request will 404 until it is` |
| Répertoire de réponses présent mais illisible | `warning: … cannot be read: … — every request will 404 until that is fixed`. La cause la plus probable est un serveur tournant sous un utilisateur qui n'est pas le propriétaire du répertoire |
| API d'administration hors boucle locale | `warning: the admin API is not bound to loopback — …` |
| `RESCRIPTUM_ANSWER_TOKEN` de moins de 16 caractères | un avertissement, **pas** une erreur — refuser de démarrer laisserait un parc incapable de s'installer |
| Tout problème dans le jeu de réponses | une ligne `warning:` chacun, le même jeu que signale `check` |
| `RESCRIPTUM_PUBLIC_HOST` non défini | La réponse de la table de routage, ou l'unique adresse d'interface s'il n'y a pas de route par défaut. Journalisé dans les deux cas, en avertissement **nommant les autres adresses** s'il y en a. Un hôte derrière du NAT se trompe toujours en silence |
| TFTP ne peut pas se lier | `warning: cannot bind TFTP on … ` — **le seul listener dont l'échec de liaison n'est pas fatal.** Le port 69 est le seul port privilégié de la conception, donc le seul bind qui puisse échouer pour quelque chose que personne n'a configuré ; les réponses sont le produit, et mourir ferait échouer toutes les installations en cours pour signaler qu'un second port n'a pas pu être ouvert. `boot check` sort en non-zéro et le message nomme les façons d'obtenir le port |
| Répertoire de médias absent ou illisible | une ligne `warning: media: …` — un parc ne doit jamais être incapable de s'installer parce qu'une image est bizarre |

## Options de compilation

| Feature | Défaut | Effet |
|---|---|---|
| `sqlite` | activée | Le store SQLite et l'API d'administration |
| `boot` | activée | Le catalogue de médias, le lecteur ISO et le listener média |

Mesuré sur ARMv7 (gnueabihf, plancher glibc 2.17), les quatre d'affilée le 2026-08-29.
Remesurez plutôt que de citer ces chiffres : ils ont bougé d'environ 375 Ko quand cette
cible est passée de musl à glibc, et le jeu qu'ils remplacent ici avait dérivé d'environ
200 Ko.

| Build | Octets |
|---|---|
| les deux (défaut) | 2 813 712 |
| `sqlite` seule | 2 557 592 |
| `boot` seule | 1 649 048 |
| aucune | 1 392 544 |

## Limites fixes

Non configurables, et délibérément :

| Limite | Valeur | Où |
|---|---|---|
| Corps de requête | 1 Mo | endpoint de réponse — un `Content-Length` invraisemblable est refusé depuis l'en-tête |
| Taille d'un document | 256 Ko | `PUT` de l'API d'administration |
| Requêtes capturées | 1000 captures | comptées depuis le répertoire au démarrage, donc un redémarrage ne repart pas de zéro |
| Échecs d'administration avant blocage | 5 en 60 s | le blocage double jusqu'à un maximum de 900 s |
| Adresses suivies par le garde-fou | 4096 | pour qu'il ne puisse pas être transformé en fuite mémoire |
| Filet de rechargement du listing | 1 s | force une relecture même quand le mtime du répertoire semble inchangé |
