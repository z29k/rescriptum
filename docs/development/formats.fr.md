---
title: Formats et fusion
description: Un type Doc par-dessus cinq parseurs, les règles de fusion, l'arbre XML, et pourquoi la substitution se fait sur des valeurs parsées.
sidebar:
  label: Formats
  order: 5
---

# Formats et fusion

`src/format/mod.rs` donne une seule interface à chaque format de document, pour que
`select.rs` n'ait jamais à savoir lequel il tient.

```rust
enum Inner {
    Toml(toml_edit::DocumentMut),
    Yaml(serde_yaml_ng::Value),
    Json(serde_json::Value),
    Xml(xml::Document),
    Text(String),
}
```

`Doc` l'enveloppe et offre `parse`, `merge`, `render`, `control`, `strip_control`,
`substitute` et `has_placeholders`. Ajouter un format, c'est ajouter une variante et remplir
ces sept-là — rien au-dessus de ce module ne change.

## `Kind`

`Kind::for_extension` est une **liste blanche** délibérée. `txt` n'y est pas, pour qu'un
fichier de notes égaré à côté des réponses ne devienne jamais un candidat.

`Kind` est la *famille* ; l'*extension* est gardée séparément, parce que ce ne sont pas la
même chose :

- **`Kind`** décide comment parser, comment fusionner, et le `Content-Type`.
- **L'extension** décide si un endpoint peut être servi, et quel validateur `check` appelle.
  `ks` et `preseed` sont tous deux `Kind::Text` mais ne partagent pas de validateur — ce qui
  est pourquoi `Resolution` porte `format_name` à côté de `format`.

Filtrer sur la famille plutôt que sur l'extension laisserait un preseed répondre à `/rhel/ks`.

## `endpoint_formats`

Une petite table d'alias associant un segment d'URL aux extensions qu'il accepte. Deux pièges
y vivent, tous deux déjà payés :

- **Filtrer sur l'extension, pas sur le `Kind`** — comme ci-dessus.
- **Un alias doit être assez spécifique pour que personne ne l'atteigne par accident.** `seed`
  a été retiré : `s=http://server/seed/` est une URL de seed NoCloud ordinaire, et elle sert
  du YAML.

Un segment ne nommant aucun alias ne contraint rien, donc `/answer` continue de fonctionner.

## Règles de fusion

| | |
|---|---|
| Maps / objets | fusionnent récursivement |
| Toute autre valeur | remplacée intégralement par la couche supérieure |
| Tableaux | **remplacent, ils ne concatènent pas** |
| `Kind::Text` | concaténation dans l'ordre des couches |

Les tableaux remplacent parce que concaténer rendrait une liste impossible à raccourcir depuis
une couche supérieure, et *« ce nœud a deux disques, pas quatre »* doit rester exprimable. La
règle est la même dans tous les formats, pour qu'on n'ait jamais à se rappeler dans lequel on
est.

`merge.rs` porte le cas TOML, et utilise `as_table_like` pour que `[table]` et
`{ inline = "table" }` fusionnent entre eux — un groupe peut employer un style et une machine
l'autre sans surprise.

Le cas texte est honnête sur le fait d'être une concaténation plutôt que de prétendre le
contraire : savoir si cela équivaut à une surcharge est l'affaire du format cible (la dernière
réponse gagne en preseed ; pas toujours en kickstart).

## L'arbre XML

`format/xml.rs` est un petit arbre construit à la main par-dessus `quick-xml`, parce
qu'aucune crate généraliste ne préserve ce qu'un document de réponse a besoin de voir préservé.

**Appariement.** Les enfants sont appariés par nom d'élément **plus un attribut
discriminant** :

```rust
const DISCRIMINATORS: [&str; 5] = ["name", "id", "key", "alias", "pass"];
```

C'est ce qui rend `<component name="Microsoft-Windows-Shell-Setup">` et
`<settings pass="specialize">` fusionnables : surcharger une `pass` laisse les autres
tranquilles.

> **Des frères répétés ne sont pas toujours une liste.** Les traiter comme telle remplaçait
> chaque `<component>` d'un unattend.xml par celui que la surcouche mentionnait. S'ils portent
> un attribut discriminant, ce sont une **collection indexée**. Le `config:type="list"`
> d'AutoYaST est respecté pour le vrai cas des listes.

**Fidélité.** Déclarations, doctypes, espaces de noms et attributs survivent à une fusion.
L'indentation d'origine et le placement des commentaires, **non** — la sortie est re-rendue,
pas rustinée.

> **quick-xml émet les références d'entité comme leurs propres événements.** Les ignorer
> soude les fragments de texte alentour : `1 &lt; 2 &amp; 3` revenait en `123`. Les entités
> numériques sont résolues ; les inconnues sont refusées plutôt que silencieusement jetées.

Il ne comprend aucun schéma. `check` appelle `xmllint` là où il est installé, et c'est
l'étendue de la garantie.

## Clés de contrôle

```rust
pub const CONTROL_KEYS: [&str; 3] = ["extends", "members", "match"];
pub const XML_CONTROL_ELEMENT: &str = "answer-meta";
pub const TEXT_DIRECTIVE: &str = "answer:";
```

Elles voyagent dans ce que chaque format permet — clés natives de premier niveau dans les
formats structurés, un élément `<answer-meta>` en XML, des directives `# answer:` (ou
`// answer:`) en texte — et `strip_control()` les retire toutes avant l'envoi de la réponse.

`Control` est la forme parsée : `extends: Option<String>`, `members: Vec<String>`,
`matchers: BTreeMap<String, String>`.

## Templating

Deux règles, toutes deux porteuses :

**La substitution se fait sur des valeurs de chaîne parsées, jamais sur le texte brut du
document.** La valeur entre dans le modèle de données du document et le sérialiseur du format
l'écrit, donc c'est le sérialiseur qui échappe. Une valeur contenant un guillemet ne peut pas
casser le TOML dans lequel elle atterrit ; une contenant `<` ne peut pas casser le XML. Un
test fait passer `a"b'c<d>e&f` dans les quatre formats structurés et **reparse la sortie**.

**Un fait manquant est une erreur, jamais une chaîne vide.** Servir `node-.example.com`
installe une machine avec un nom d'hôte cassé et personne ne le remarque avant plus tard. Les
caractères de contrôle sont refusés pour la même classe de raison — un saut de ligne dans une
valeur kickstart injecte une directive dans un fichier que l'installateur exécute.

`Group::has_placeholders` est pourquoi un groupe sans template ne coûte aucun parsing par
requête : la chaîne préparée au chargement est servie telle quelle.

## Les exemples travaillés font partie de la conception

[`examples/`](https://github.com/z29k/rescriptum/tree/main/examples) porte un exemple commenté
de **chacune des treize extensions de la liste blanche**, et

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

les exerce tous. **Gardez-le ainsi.** Ils sont le seul endroit où les formats sont montrés en
train de se composer ensemble, et deux d'entre eux — `suse-node.autoyast` et
`windows-node.unattend` — sont ce qui a attrapé le doctype manquant et la `pass` non appariée.
