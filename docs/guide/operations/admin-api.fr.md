---
title: L'API d'administration
description: Gérer les réponses en HTTP — sur son propre listener, en SQLite uniquement, avec une écriture qui ne peut jamais laisser le jeu de réponses cassé.
sidebar:
  label: API d'administration
  order: 6
---

# L'API d'administration

Avec `RESCRIPTUM_STORE=sqlite`, les réponses peuvent être gérées en HTTP plutôt qu'en
éditant des fichiers. Elle est **désactivée sauf si vous la configurez**, et elle tourne sur
son propre listener.

```console
$ export RESCRIPTUM_STORE=sqlite RESCRIPTUM_DB_PATH=/srv/answers.db
$ export RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
$ export RESCRIPTUM_ADMIN_TOKEN=$(openssl rand -hex 24)
$ rescriptum
2026-08-24T08:52:30Z - admin API listening on 127.0.0.1:8001
2026-08-24T08:52:30Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=sqlite:/srv/answers.db …
```

## Trois propriétés porteuses

**1. Son propre listener.** L'endpoint de réponse n'est pas authentifié par nécessité —
l'installateur n'a aucun identifiant à offrir. Cette API décide du mot de passe root et des
clés SSH de chaque machine installée ensuite. Elle ne partage jamais ce port.

**2. SQLite uniquement.** Au-dessus d'un répertoire de fichiers il y aurait deux façons de
changer la même configuration, à la main et par le réseau, en concurrence.

**3. Une écriture ne peut jamais laisser le jeu de réponses cassé.** Chaque écriture prend un
instantané des problèmes courants, s'applique, puis compare. Tout ce qui est **nouvellement**
cassé est annulé et répondu `409`.

Le serveur **refuse de démarrer** — en erreur, pas en avertissement — si vous pointez l'API
d'administration sur le store fichiers, omettez le jeton, ou définissez un jeton de moins de
16 caractères.

## Endpoints

| Requête | Rôle |
|---|---|
| `GET /machines`, `GET /groups` | lister les identifiants |
| `GET /machines/{id}`, `GET /groups/{name}`, `GET /default` | le document stocké, **tel qu'écrit** — commentaires et mise en forme intacts |
| `PUT /machines/{id}`, `PUT /groups/{name}`, `PUT /default` | stocker un document (le corps est le document) |
| `DELETE /machines/{id}`, `DELETE /groups/{name}`, `DELETE /default` | en supprimer un |
| `GET /resolve/{id}` | la réponse **fusionnée** que cette machine recevrait |
| `GET /check` | les problèmes actuels, le même jeu que la sous-commande `check` |
| `GET /health` | vivacité — le seul endpoint sans jeton, et jamais bloqué |

Chaque endpoint nommant un document prend **`?format=`** — l'extension du document. Il vaut
`toml` par défaut, ce que ce serveur servait à ses débuts :

```console
$ curl -H "$AUTH" -X PUT --data-binary @base.preseed \
    'http://127.0.0.1:8001/groups/base?format=preseed'
```

Comme la clé d'un document est [(identifiant, format)](../answers/formats.md), un identifiant
apparaît dans `GET /machines` **une fois par format dans lequel il existe** — une machine qui
est à la fois un nœud Proxmox et un nœud Debian est listée deux fois.

## Exemples

```console
$ AUTH="Authorization: Bearer $RESCRIPTUM_ADMIN_TOKEN"

$ curl -s -H "$AUTH" http://127.0.0.1:8001/groups
{"group":["base","example-rack","rhel-compute","ubuntu-web"]}

$ curl -s -H "$AUTH" -X PUT --data-binary @rack-a.toml \
    http://127.0.0.1:8001/groups/rack-a
{"status":"stored","problems":[]}

$ curl -s -H "$AUTH" http://127.0.0.1:8001/resolve/98:fa:9b:50:d8:10
[global]
country = "fr"
keyboard = "fr"
…
```

`GET /resolve` renvoie aussi l'en-tête de réponse **`X-Answer-Source`**, portant la même
description que la ligne de log :

```
x-answer-source: format=toml machine=98fa9b50d810 group=example-rack
```

### Répéter une vraie requête

`GET /resolve` accepte les mêmes étiquettes que porterait une vraie requête, ce qui permet de
répéter une URL particulière — la différence entre `/user-data` et `/meta-data`, par exemple :

```console
$ curl -s -H "$AUTH" 'http://127.0.0.1:8001/resolve?path=/rhel/ks&serial=7ABC123'
```

**Quand une query string est présente, l'identifiant du chemin est ignoré** — les faits
viennent de la query seule. Donc `GET /resolve/98:fa:9b:50:d8:10?format=toml` ne résout
*rien*, parce que `format=toml` n'est pas une identité. Utilisez la forme sans query, ou
mettez l'identité dans la query : `?mac=98:fa:9b:50:d8:10`.

## Elle ne vous laissera pas casser le parc

Chaque écriture est vérifiée après application. Si elle a introduit un problème — un cycle
entre groupes, un document référençant un groupe qui n'existe plus — l'écriture est
**annulée** et vous recevez un `409` disant ce que vous avez cassé :

```console
$ curl -s -H "$AUTH" -X DELETE 'http://127.0.0.1:8001/groups/base?format=preseed'
{"error":"refused: this would break the answer set (rolled back)",
 "problems":["machine \"98fa9b50d810\": extends unknown group \"base\""]}
```

Deux choses découlent de ce fonctionnement :

- **Une écriture réussie signale quand même les problèmes *préexistants***, dans le tableau
  `problems`. Une réponse propre n'implique jamais que tout le jeu est sain — seulement que
  vous ne l'avez pas aggravé.
- C'est pourquoi un `extends` de machine pointant sur un groupe manquant est détecté au
  **chargement** plutôt qu'au moment où cette machine demande. Le garde-fou ne peut attraper
  que ce que le rapport de problèmes connaît.

Les documents malformés sont également refusés à l'écriture, plutôt que de devenir un `500`
la prochaine fois qu'une machine les demande :

```console
$ curl -s -H "$AUTH" -X PUT --data-binary 'x = = 1' http://127.0.0.1:8001/machines/aa-bb-cc-dd-ee-01
{"error":"document: invalid TOML: TOML parse error at line 1, column 5 …"}
```

## Identifiants

Lettres, chiffres et `- _ . :` uniquement. Ils deviennent des **noms de répertoires** sous
`export` et dans le store fichiers, donc tout ce qui pourrait traverser un répertoire est
rejeté — à la frontière de l'API *et* dans les deux stores. `groups` et `default` sont
réservés comme identifiants de machine pour la même raison : ce sont les répertoires que
l'agencement garde pour lui, et une base qui en accepterait un exporterait vers un répertoire
incapable de le contenir.

## Codes de statut

| Code | Signifie |
|---|---|
| `200` | fait |
| `400` | document malformé, identifiant invalide, ou corps non-UTF-8 |
| `401` | jeton manquant ou faux |
| `404` | document ou endpoint inexistant ; rien ne se résout pour cet identifiant |
| `409` | l'écriture aurait cassé le jeu de réponses (annulée), ou un `resolve` qui n'a pas pu rendre |
| `413` | document de plus de 256 Ko |
| `429` | cette adresse est bloquée après des échecs d'authentification répétés |
| `500` | le store n'a pas pu être lu ou écrit |

## Veiller sur le jeton

Le jeton constitue toute l'authentification, et ce qu'il protège mérite d'être dit
franchement : les documents de réponse portent `root-password-hashed` et `root-ssh-keys`,
donc **quiconque peut écrire sur cette API décide des identifiants root de chaque machine que
vous installerez ensuite**.

Générez-en un vrai — pas un mot auquel vous avez pensé :

```console
$ openssl rand -hex 24        # ou : head -c 24 /dev/urandom | base64
```

**Ne le mettez pas sur une ligne de commande.** Tout ce qui se trouve dans les arguments d'un
processus est visible par tous les autres utilisateurs via `ps`, ce qui inclut le mettre
directement dans une tâche planifiée DSM. Gardez-le dans un fichier réservé à root et
sourcez-le — voir
[Sécurité](./security.md#ne-mettez-pas-un-jeton-sur-une-ligne-de-commande).

Ce que le serveur fait de son côté :

- **Compare le jeton en temps constant**, pour qu'il ne puisse pas être récupéré octet par
  octet par qui chronomètre les réponses.
- **Exclut une adresse qui insiste.** Cinq échecs en une minute valent un blocage, doublant à
  chaque récidive jusqu'à un maximum de quinze minutes, et chaque tentative est journalisée.
  Le blocage s'applique aussi à un jeton **correct** venant de cette adresse — sinon deviner
  jusqu'à tomber juste ne coûterait rien.
- **Borne sa propre comptabilité** à 4096 adresses suivies, pour que le garde-fou ne puisse
  pas lui-même être transformé en fuite de mémoire.
- **Laisse `GET /health` non authentifié et non bloqué**, pour que la supervision ne
  s'éteigne pas pendant une attaque.

```
2026-08-24T08:52:32Z - admin: 10.0.0.9 failed authentication 5 times — blocked for 60s
```

## Deux limites à prévoir

- **Elle parle HTTP en clair**, donc le jeton traverse le réseau en clair. Sur la boucle
  locale c'est sans objet. Ailleurs, mettez un reverse proxy terminant TLS devant.
- **Le blocage par adresse n'arrête pas un attaquant disposant de nombreuses adresses.** C'est
  la longueur du jeton qui rend la devinette sans espoir — d'où le plancher de 16 caractères
  au démarrage.

Binder au-delà de la boucle locale est votre choix, et le serveur le dit dans le log quand
vous le faites :

```
2026-08-24T08:52:30Z - warning: the admin API is not bound to loopback — it rewrites what gets installed on every machine, so restrict it to a management network
```

`127.0.0.1` plus un tunnel SSH est le défaut sûr.

## Voir aussi

- [Le store SQLite](./sqlite.md) — le prérequis.
- [Comment le garde-fou est construit](../../development/admin.md) — les internes.
