---
title: Internes de l'API d'administration
description: Son propre listener, un jeton comparé en temps constant, un garde-fou à backoff exponentiel, et une écriture qui s'annule elle-même.
sidebar:
  label: API d'administration
  order: 7
---

# Internes de l'API d'administration

`src/admin.rs`, activée seulement par `RESCRIPTUM_ADMIN_ADDR`, et seulement au-dessus de
SQLite. Trois propriétés sont porteuses — **un changement qui en supprime discrètement une est
une régression**.

## 1. Son propre listener

L'endpoint de réponse n'est pas authentifié par nécessité : l'installateur n'a aucun
identifiant à offrir. Cette API décide du mot de passe root et des clés SSH de chaque machine
installée ensuite. Elle ne partage jamais ce port.

`Config::validate` refuse de démarrer — en erreur, pas en avertissement — sans jeton, avec un
jeton de moins de 16 caractères, ou au-dessus du store fichiers. Ces vérifications tournent
*avant* que le listener ne soit bindé, donc une mauvaise configuration n'est jamais brièvement
en ligne.

## 2. SQLite uniquement

Au-dessus d'un répertoire de fichiers il y aurait deux façons de changer la même
configuration, à la main et par le réseau, en concurrence l'une avec l'autre.

## 3. L'écriture qui ne peut pas casser le parc

```rust
fn guarded(admin, kind, id, format, body) -> Response<Body> {
    let before   = admin.answers.problems()?;   // instantané des dégâts
    let previous = admin.store.snapshot()?…;    // ce qui était là, pour pouvoir le restaurer
    let existed  = apply(…)?;                   // put ou delete
    let after    = admin.answers.problems();
    let introduced = after.filter(|p| !before.contains(p));
    if !introduced.is_empty() { restore(previous); return 409 }
    200 avec `problems: before`
}
```

- Seuls les problèmes **nouvellement** introduits déclenchent l'annulation. Un store déjà cassé
  reste éditable — sinon un mauvais état serait impossible à réparer par l'API qui l'a causé.
- Une écriture réussie signale quand même les problèmes **préexistants**, pour qu'une réponse
  propre n'implique jamais que tout le jeu est sain.
- C'est pourquoi un `extends` de machine pointant sur un groupe manquant est détecté au
  [**chargement**, dans `select.rs`](./selection.md#construire-un-listing), plutôt qu'au moment
  où cette machine demande. **Le garde-fou ne peut attraper que ce que `problems()` signale.**
  Ajouter une nouvelle classe de casse veut dire l'ajouter là, sinon le garde-fou cesse
  silencieusement de la couvrir.

Les documents malformés sont refusés à l'écriture (`400`) plutôt que de devenir un `500` la
prochaine fois qu'une machine en demande un.

## Authentification

**Comparaison en temps constant.** Un `==` ordinaire retourne dès le premier octet différent,
ce qui fuit le jeton octet par octet à qui chronomètre les réponses — quelques milliers de
requêtes plutôt qu'un nombre impossible. Comparer tous les octets quoi qu'il arrive supprime
le signal. Sur un réseau le timing se perd généralement dans la gigue, donc c'est une
précaution ; ça coûte cinq lignes.

**`AuthGuard`** exclut une adresse après des échecs répétés :

| Constante | Valeur |
|---|---|
| `MAX_FAILURES` | 5 |
| `FAILURE_WINDOW` | 60 s |
| `BASE_BLOCK` | 60 s, doublant à chaque récidive |
| `MAX_BLOCK` | 900 s |
| `MAX_TRACKED` | 4096 adresses |

Trois détails qui ne sont pas des accidents :

- **Le blocage s'applique aussi à un jeton *correct*.** Sinon deviner jusqu'à tomber juste ne
  coûterait rien.
- **`MAX_TRACKED` est borné**, pour que le garde-fou ne puisse pas lui-même être transformé en
  fuite de mémoire par un attaquant qui fait tourner ses adresses sources.
- **`GET /health` est vérifié avant le garde-fou et avant l'auth**, pour que la supervision ne
  s'éteigne pas pendant une attaque.

Un blocage répond `429` avec `Retry-After`.

## Traitement des requêtes

```rust
let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
match (&method, segments.as_slice()) {
    (&Method::GET,    ["machines"])      => list(…),
    (&Method::GET,    ["resolve", id])   => resolve(…),
    (&Method::PUT,    ["groups", id])    => put(…).await,
    …
    _ => error(NOT_FOUND, "no such endpoint"),
}
```

`?format=` choisit l'extension du document, `toml` par défaut — ce que ce serveur servait à ses
débuts.

> **Lire le corps de la requête avant de la rejeter.** Répondre et fermer pendant que le
> client écrit encore lui vaut un `ECONNRESET` au lieu de la réponse. `put()` draine d'abord,
> puis valide l'identifiant.

> **Chaque réponse d'administration doit définir `Connection: close`.** Sans cela, chaque
> client de test attendait l'expiration de la connexion — la suite prenait 30 s au lieu de
> 0,4 s — et la coupure finale arrivait parfois comme un reset plutôt qu'un EOF propre.

`GET /resolve` définit `X-Answer-Source` avec la même description que la ligne de log.

> **`GET /resolve/{id}` ignore l'identifiant du chemin quand une query string est présente** —
> les faits viennent de la query seule, ce qui permet de répéter une vraie requête. Cela rend
> `?format=toml` sur cet endpoint activement faux : il ne résout rien. Documenté dans le
> [guide](../guide/operations/admin-api.md#répéter-une-vraie-requête).

## Identifiants

`valid_id` — lettres, chiffres, `- _ . :`, aucun séparateur de chemin — est imposé **à la
frontière de l'API et dans les deux stores**. `export` retransforme les identifiants en noms
de fichiers, donc tout ce qui pourrait traverser un répertoire doit être rejeté dans la couche
qui construit le chemin, pas seulement dans celle qui l'a reçu.

## Connu et accepté

- **La limitation par adresse n'arrête pas un attaquant disposant de nombreuses adresses.**
  C'est la *longueur* du jeton qui rend la devinette sans espoir — d'où le plancher de 16
  caractères au démarrage.
- **Elle parle HTTP en clair.** Mettez du TLS devant si elle quitte la boucle locale.
- **Binder au-delà de la boucle locale journalise un avertissement** plutôt que de refuser,
  parce qu'un réseau d'administration est un choix légitime.

## Tests

`tests/admin.rs` (15 cas) couvre le routage, l'annulation, la validation des identifiants et
les codes de statut. `tests/guards.rs` (5) couvre l'arithmétique du verrouillage et le fait que
`/health` reste joignable à travers.
