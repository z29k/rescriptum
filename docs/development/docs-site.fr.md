---
title: Le site de documentation
description: Comment docs/ est écrit, relu avec notabene, gardé par la CI, et publié sur GitHub Pages.
sidebar:
  label: Site de documentation
  order: 12
---

# Le site de documentation

Ce site **est** `docs/` dans le dépôt, rendu par
[notabene](https://z29k.github.io/notabene/) et publié sur GitHub Pages. Le binaire Rust n'en
sait rien ; la chaîne d'outils de doc, c'est un `package.json` et un fichier de configuration,
et la retirer laisserait `docs/` comme du Markdown parfaitement lisible.

## Pourquoi un site et pas un README plus long

Le README avait atteint 28 Ko et était trois documents portant un seul manteau : une
présentation, un manuel utilisateur, et une note d'architecture. Un lecteur cherchant l'étape
du pare-feu DSM devait faire défiler la sémantique de fusion. Donc :

- **`docs/guide/`** — utiliser rescriptum : installer, écrire des réponses, l'exploiter en
  production.
- **`docs/development/`** — construire rescriptum : les contraintes, les internes, la release.
- **`README.md`** — ce que c'est, une démonstration de 30 secondes, et des liens vers le site.

Les deux espaces ont des publics différents et aucune raison de s'entremêler.

## Deux langues

Le site est bilingue : **l'anglais est la source**, le français en est une traduction. La
stratégie i18n `suffix` fait que les fichiers anglais gardent leurs chemins et leurs URL, et
que les français sont des voisins `*.fr.md` :

```
docs/guide/answers/grouping.md      → /guide/answers/grouping
docs/guide/answers/grouping.fr.md   → /fr/guide/answers/grouping
```

Cet agencement a été choisi plutôt qu'un dossier par langue parce qu'il s'ajoute à une doc
existante sans rien déplacer — les URL anglaises et leurs fils de commentaires survivent.

Les règles qui en découlent :

- **Écrire l'anglais d'abord**, puis traduire. Un changement sur une page anglaise qui n'est
  pas répercuté laisse la page française périmée plutôt que cassée ; le lecteur retombe dessus
  avec un bandeau.
- **Les liens gardent le nom de base.** Depuis une page française, écrivez `./selection.md`,
  pas `./selection.fr.md` — notabene résout la langue. En revanche, **les ancres doivent être
  le slug du titre français** : `./templating.md#machine-exige-un-document-machine`.
- **Les commentaires sont par langue.** Un commentaire laissé sur la page française est son
  propre fil et pointe sur le fichier source français.
- L'habillage du site, la recherche et `llms.txt` sont également par langue.

`README.md` et `README.fr.md` suivent la même règle et se lient l'un à l'autre.

## Travailler sur la doc

```bash
npm install          # une fois
npm run docs         # → http://localhost:3009
```

Cela ouvre le site avec la **boucle de revue** activée : sélectionnez n'importe quel texte sur
la page rendue et laissez un commentaire exactement là où est le problème. Le commentaire
ancré *est* l'instruction — pas besoin de citer un passage dans une fenêtre de chat en espérant
que l'agent le retrouve.

Dites ensuite à votre agent **« traite les commentaires de la doc »**. Il lit
`docs/.notabene/`, édite la source, marque chaque commentaire traité, et ajoute une entrée de
journal disant ce qui a changé et pourquoi.

| Script | Rôle |
|---|---|
| `npm run docs` | le serveur de revue, avec rechargement à chaud |
| `npm run docs:build` | le site statique public dans `./_site` |
| `npm run docs:preview` | servir ce qui a été construit |
| `npm run docs:lint` | valider chaque lien interne contre les routes émises par le dernier build |
| `npm run docs:status` / `docs:stop` | gérer un serveur de dev détaché |

Vous utilisez Claude Code ? `/plugin marketplace add z29k/notabene` puis
`/plugin install notabene@z29k`, et dites *« set up notabene »*. Le plugin lance son propre
renderer épinglé, donc il n'entre pas en conflit avec celui du `package.json`.

### Le mode de revue est `approve`

`notabene.config.mjs` définit `review: "approve"`, donc l'agent **propose** au lieu de
résoudre : chaque édition est validée contre son **vrai diff git** sur `/review` avant que le
commentaire ne soit clos. Une documentation qui décrit des mots de passe root et de la
configuration de démarrage mérite d'être lue avant d'être publiée. Passez à `"auto"` si cette
cérémonie ne se justifie pas.

## Écrire une page

Chaque page est du CommonMark avec un frontmatter YAML optionnel :

```yaml
---
title: Groupes et fusion
description: Une phrase — elle devient la meta description et l'extrait de recherche.
sidebar:
  label: Groupement       # texte de la barre latérale, si le titre est trop long
  order: 3                # position parmi les frères, croissante
---
```

Tout a une valeur par défaut : une page sans frontmatter se rend très bien, triée
alphabétiquement. Un dossier est nommé et positionné par son `index.md`.

Conventions dans ce dépôt :

- **Liens relatifs entre pages**, avec l'extension `.md` — `./selection.md`,
  `../reference/configuration.md`. Ils deviennent des routes sur le site et restent
  cliquables sur GitHub.
- **URL GitHub absolues pour les fichiers du dépôt** — `answers/`, `CLAUDE.md`, un workflow.
  Ils sont hors de `docs/` et n'ont pas de route.
- **Diagrammes Mermaid** rendus nativement, dans une clôture ` ```mermaid ` — voir
  [architecture](./architecture.md) et
  [le cycle de vie d'une requête](./request-lifecycle.md).
- **Chaque page a besoin de son voisin `*.fr.md`**, avec le frontmatter traduit aussi — le
  `title`, la `description` et le `sidebar.label` sont tous visibles par le lecteur.

## Configuration

`notabene.config.mjs` à la racine du dépôt. Les parties qui comptent :

```js
roots: [
  { key: "guide",       label: "Guide",                              path: "docs/guide" },
  { key: "development", label: { en: "Development", fr: "Développement" }, path: "docs/development" },
],
store: "docs/.notabene",
home: { en: "docs/home.md", fr: "docs/home.fr.md" },
i18n: { locales: ["en", "fr"], defaultLocale: "en", strategy: "suffix" },
branding: {
  logo: "assets/rescriptum-logo.jpg",
  favicon: "assets/rescriptum-logo.jpg",
  socialImage: "assets/rescriptum-logo.jpg",
},
editPattern: "https://github.com/z29k/rescriptum/edit/develop/{path}",
review: "approve",
publish: { site: "https://z29k.github.io", base: "/rescriptum" },
```

Chaque chaîne visible par le lecteur accepte une map par langue — le `label` et la
`description` d'un espace, la page d'accueil, chaque libellé de lien de navigation, le titre du
bloc latéral, le pied de page. Non définie pour une langue, elle retombe sur la langue par
défaut.

Le logo est `assets/rescriptum-logo.jpg` : un rescrit scellé sur une disquette — une réponse
écrite, remise par une machine. Il sert de logo dans la barre du haut, de favicon et de carte
sociale, et c'est aussi l'image qu'utilise le README.

`editPattern` pointe sur **`develop`**, pas `main` : la doc y est fusionnée comme tout le
reste, et `main` ne reçoit que des commits de release.

`docs/.notabene/` est le store de commentaires et de journal — du JSON simple, commité,
diffable dans une PR. Commitez-le.

## Le garde-fou CI

`.github/workflows/ci.yml` a un job `docs` : `npm ci`, construction du site public, puis
`notabene lint`, qui vérifie chaque lien interne contre les routes que le build a **réellement
émises** et suggère les quasi-correspondances. Un lien mort dans de la documentation publiée
est bon marché à empêcher et gênant à livrer.

Il tourne sur les mêmes pushes que les garde-fous Rust.

## Publication

`.github/workflows/docs.yml` construit en `--public` et déploie sur GitHub Pages à chaque push
sur **`main`** touchant `docs/`, la configuration ou le workflow — plus `workflow_dispatch`,
pour publier une correction de doc sans attendre une release.

Comme `main` ne reçoit que des commits de release, **la documentation part normalement avec
une release.** Lancez le workflow à la main quand elle ne doit pas attendre.

L'artefact est le build public en lecture seule : pas d'interface de revue, pas de données de
store, plus `llms.txt`, un jumeau Markdown par page, un sitemap et des métadonnées OpenGraph.
`pagefind` est une dépendance de développement, donc `npm ci` donne au site une recherche
plein texte sans configuration supplémentaire.

Mise en place unique du dépôt : **Settings → Pages → Source = GitHub Actions**.

## La situation des dépendances

`npm audit` signale des avis de sécurité dans Astro, esbuild et sharp, transitivement sous
notabene, sans correctif disponible en amont pour l'instant. Ils sont **limités au
développement** : rien de `node_modules` n'est exécuté par le site publié ni n'atteint le
binaire Rust, et le job CI construit du HTML statique à partir de Markdown que ce dépôt possède.

À revérifier quand notabene se met à jour, pas à bloquer dessus.
