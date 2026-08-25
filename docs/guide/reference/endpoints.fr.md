---
title: Surface HTTP
description: Méthodes, chemins, codes de statut, en-têtes et limites — pour les deux listeners.
sidebar:
  label: Surface HTTP
  order: 2
---

# Surface HTTP

Deux listeners, et ils ne partagent jamais un port. L'endpoint de réponse est ce à quoi
parlent les installateurs ; l'[API d'administration](../operations/admin-api.md) est
désactivée sauf configuration.

## L'endpoint de réponse

| Requête | Réponse |
|---|---|
| `POST` **n'importe quel chemin** | la réponse, typée par son format |
| `GET` **n'importe quel chemin** | la même chose |
| `GET /health` | `200 OK`, corps `OK\n` — pas de jeton nécessaire, jamais limité en débit |
| toute autre méthode | `405` |

**N'importe quel chemin**, parce que l'URL est gravée dans une ISO et que ce serveur n'a pas
son mot à dire. Le chemin n'est pas ignoré pour autant : les segments nommant un
[alias de format](./formats.md) restreignent les documents pouvant répondre, et le chemin
fournit aussi les [faits](../answers/selection.md#les-faits-quun-sélecteur-peut-tester)
`path`, `file` et `segment`.

### Codes de statut

| Code | Quand |
|---|---|
| `200` | une réponse s'est appliquée |
| `400` | le corps n'a pas pu être lu |
| `401` | `RESCRIPTUM_ANSWER_TOKEN` est défini et la requête ne l'a pas présenté |
| `404` | rien n'a revendiqué la requête et il n'y a pas de `default` pour le format demandé |
| `405` | une méthode autre que `GET` ou `POST` |
| `413` | corps de plus de 1 Mo, ou `Content-Length` en annonçant un |
| `500` | un document ne parse pas, un groupe manque, un template n'a pas pu être rempli, ou la recherche a paniqué |
| `503` | à `RESCRIPTUM_MAX_CONNECTIONS` — écrit immédiatement, puis la connexion se ferme |

### En-têtes de réponse

| En-tête | Valeur |
|---|---|
| `Content-Type` | selon le format de la réponse — voir la table ci-dessous |
| `Content-Length` | toujours défini |
| `Connection` | `close` |
| `WWW-Authenticate` | `Bearer`, sur un `401` |

| Format | `Content-Type` |
|---|---|
| `toml`, et tous les formats texte (`ks`, `preseed`, `cfg`, `seed`, `ipxe`) | `text/plain; charset=utf-8` |
| `yaml`, `yml` | `text/yaml; charset=utf-8` |
| `json`, `ign` | `application/json` |
| `xml`, `autoyast`, `unattend` | `application/xml; charset=utf-8` |

Le TOML est servi en `text/plain` plutôt qu'en `application/toml` parce que c'est ce
qu'attend l'installateur Proxmox.

### Limites de traitement des requêtes

| | |
|---|---|
| **Plafond du corps** | 1 Mo. Un `Content-Length` invraisemblable est refusé **depuis l'en-tête**, avant toute lecture — plutôt que d'allouer pour lui et de faire sauter une limite plus tard |
| **Délai de lecture des en-têtes** | `RESCRIPTUM_TIMEOUT_SECS`, 10 s par défaut |
| **Échéance de la connexion entière** | la même valeur. Les deux sont nécessaires : le délai d'en-têtes s'arrête à la fin des en-têtes, donc un client qui promet un corps sans l'envoyer garerait sinon une connexion indéfiniment |
| **Concurrence** | `RESCRIPTUM_MAX_CONNECTIONS` en vol ; au-delà, un `503` et fermeture plutôt qu'une mise en file |
| **Authentification** | seulement quand `RESCRIPTUM_ANSWER_TOKEN` est défini. Comparée en temps constant. Les échecs sont journalisés et **jamais** limités en débit |

### Authentification

```
Authorization: Bearer <RESCRIPTUM_ANSWER_TOKEN>
```

Proxmox l'envoie quand son ISO a été préparée avec `--answer-auth-token`. Rien d'autre ne le
peut, ce qui est pourquoi c'est désactivé par défaut. Voir
[Sécurité](../operations/security.md).

## L'API d'administration

Un listener séparé, `RESCRIPTUM_ADMIN_ADDR`, en SQLite uniquement. Détails complets sur
[sa propre page](../operations/admin-api.md).

| Requête | Rôle |
|---|---|
| `GET /machines`, `GET /groups` | lister les identifiants |
| `GET /machines/{id}`, `GET /groups/{name}`, `GET /default` | le document stocké, tel qu'écrit |
| `PUT /machines/{id}`, `PUT /groups/{name}`, `PUT /default` | stocker un document |
| `DELETE /machines/{id}`, `DELETE /groups/{name}`, `DELETE /default` | en supprimer un |
| `GET /resolve/{id}` | la réponse fusionnée que cette machine recevrait |
| `GET /check` | les problèmes actuels |
| `GET /health` | vivacité — pas de jeton, jamais bloqué |

Tous les endpoints de document prennent `?format=<ext>`, `toml` par défaut.

| Code | Quand |
|---|---|
| `200` | fait |
| `400` | document malformé, identifiant invalide, ou corps non-UTF-8 |
| `401` | jeton manquant ou faux |
| `404` | document ou endpoint inexistant ; rien ne se résout pour cet identifiant |
| `409` | l'écriture aurait cassé le jeu de réponses (annulée), ou un `resolve` qui n'a pas pu rendre |
| `413` | document de plus de 256 Ko |
| `429` | cette adresse est bloquée ; `Retry-After` dit pour combien de temps |
| `500` | le store n'a pas pu être lu ou écrit |

Chaque réponse d'administration définit `Connection: close`. Un `GET /resolve` réussi définit
aussi `X-Answer-Source`, portant la même description que la ligne de log.

## Journalisation

Une ligne par requête, sur **stderr** par défaut. `RESCRIPTUM_LOG` choisit ce qui est gardé
et `RESCRIPTUM_LOG_FILE` où cela va — voir
[journalisation](./configuration.md#journalisation).

```
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=example-rack bytes=431
```

Les lignes de niveau serveur portent `-` là où serait l'adresse du pair. Voir
[dépannage](../operations/troubleshooting.md#lire-une-ligne).
