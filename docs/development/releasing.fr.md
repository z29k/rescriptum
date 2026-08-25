---
title: Branches et releases
description: develop ne publie rien ; un tag sur main produit tous les binaires. La séquence exacte, et ce que la CI refuse.
sidebar:
  label: Releases
  order: 10
---

# Branches et releases

Le modèle reprend délibérément celui du projet frère `notabene` — même mainteneur, mêmes
attentes.

## Branches

| Branche | Règle |
|---|---|
| **`main`** | stable. Seuls les commits de release et les tags `vX.Y.Z` y atterrissent. Ne poussez jamais de travail de fonctionnalité directement |
| **`develop`** | intégration. Maintenue à la prochaine version en cours |
| **`feature/<nom>`**, **`fix/<nom>`** | partent de `develop`, PR de retour vers `develop` |

```
main ──●────────────────────────●─(tag vX.Y.Z)──▶  releases
        \                      /
develop  ●───●───●───●───●────●  ────────────────▶  garde-fous CI seulement, ne publie rien
          \     /   \       /
    feature/…  ●   fix/… ●          (PR vers develop)
```

**`develop` ne publie rien.** Elle lance les garde-fous — build, tests, clippy, fmt — et
s'arrête là. Pas de préversions, pas d'artefacts. Les binaires sont produits **uniquement** par
un tag `vX.Y.Z` sur `main`.

C'est la seule chose qui ne se reporte *pas* depuis notabene, qui est un paquet npm et publie
des préversions sur un dist-tag `@dev`. Ce projet livre un binaire compilé, donc l'artefact de
release est une GitHub Release avec des binaires compilés en croisé attachés, construits par
une matrice CI.

## Commits

Commits conventionnels **avec un scope** :

```
feat(http): answer GET as well as POST
fix(select): normalize member strings before comparing
chore: release v0.2.0
```

Gardez les PR ciblées. Ajouter une dépendance exige une raison dans le message de commit — ce
binaire tourne en root sur le matériel d'autres gens.

## Faire une release

```bash
# sur develop, tout étant vert
$EDITOR Cargo.toml          # monter la version
cargo build                 # rafraîchir Cargo.lock
git commit -am "chore: release vX.Y.Z"

git checkout main && git merge --no-ff develop
git tag -a vX.Y.Z -m "rescriptum vX.Y.Z"
git push origin main --follow-tags
```

`.github/workflows/release.yml` ensuite :

1. **Refuse le tag s'il diverge de `Cargo.toml`.** Une release dont le binaire annonce une
   version différente de son tag est un problème de support qui survit à la release.
2. Compile en croisé les [cinq cibles publiées](./building.md#les-cibles-de-release).
3. Empaquette chacune en `rescriptum-<version>-<cible>.tar.gz`, avec `README.md` et `LICENSE` à
   côté du binaire, plus une **somme SHA-256** — qui fait tourner cela en root devrait pouvoir
   vérifier ce qu'il a téléchargé.
4. Crée la GitHub Release avec `gh` et `--generate-notes`.

Il est relançable à la main via `workflow_dispatch` avec un tag, pour quand un job échoue après
que le tag est déjà poussé.

Chaque action utilisée est une action officielle `actions/*`, et `gh` est déjà sur le runner.
C'est délibéré, pour la même raison que tout le reste de cette page.

## Versionnage

SemVer. Le tag est `vX.Y.Z` et doit correspondre exactement à `Cargo.toml`.

Les documents de réponse sont des données, pas de l'état : rien ne migre, et un nouveau binaire
lit le même répertoire. L'exception est le **schéma SQLite**, qui porte un `user_version` —
voir [stores](./stores.md#le-store-sqlite). Il n'y a qu'une version pour l'instant. En ajouter
une seconde veut dire écrire l'étape de migration *et* une montée mineure au minimum, et les
notes de version doivent le dire, parce qu'un binaire plus ancien refusera la base mise à jour
plutôt que de la lire à moitié.

## Documentation

Le [site de documentation](./docs-site.md) est publié depuis **`main`**, donc un changement de
doc part avec la prochaine release — ou en lançant le workflow `docs` à la main
(`workflow_dispatch`) quand il ne doit pas attendre.
