---
title: Internes de la sélection
description: Normalisation, faits, scoring, cache du listing — le module avec le plus de comportement par ligne.
sidebar:
  label: Sélection
  order: 4
---

# Internes de la sélection

`src/select.rs` et `src/facts.rs` portent le comportement qui compte. Les deux sont de la
logique pure sur des données qu'on leur passe, et les deux sont abondamment testés — 27 et 22
tests respectivement.

## Normalisation

```rust
pub fn normalize(input: &[u8]) -> String   // alphanumériques ASCII minuscules, le reste jeté
```

Elle prend des **octets, pas un `&str`**, volontairement : un corps de requête est constitué
d'octets arbitraires et n'a pas besoin d'être de l'UTF-8 valide. Filtrer sur les
alphanumériques ASCII contourne complètement la question — pas de validation, pas de
conversion avec perte, pas de mode d'échec.

C'est ce qui rend la correspondance indifférente au style de séparateur et à la façon dont
Proxmox structure son JSON cette version-ci. C'est un test de sous-chaîne sur des octets, pas
un schéma.

> **`normalize_pattern` est l'autre.** La normalisation ordinaire retire `*` et `?` avec le
> reste de la ponctuation, ce qui transforme chaque glob en littéral — en silence. Les motifs
> de sélecteur doivent passer par `normalize_pattern`, qui les conserve.

## Faits

`Facts` est une map étiquette → valeurs, plus la botte de foin. Trois sources, étagées du plus
structuré au moins structuré :

**Les paramètres de query** — parsing fait main avec décodage pourcent, plutôt que de tirer une
crate d'URL pour vingt lignes de travail. Les valeurs vont **aussi dans la botte de foin**,
pour qu'un document nommé d'après une MAC résolve que la MAC soit arrivée dans un corps POST
ou dans une query string. Sans cela, un `GET` — qui n'a aucun corps — ne pourrait jamais
correspondre par nom.

**Le chemin** apporte trois étiquettes synthétisées :

| Étiquette | Depuis |
|---|---|
| `path` | le chemin entier, débarrassé de ses slashes |
| `file` | son dernier segment |
| `segment` | chaque segment, comme valeurs séparées |

`file` n'est pas de la décoration : la source de données NoCloud de cloud-init récupère
`user-data` **et** `meta-data` depuis une seule URL et ignore complètement la source si l'un
des deux manque, donc le même serveur doit leur répondre différemment. Les segments de chemin
alimentent aussi la botte de foin, parce que NoCloud peut développer
`__dmi.chassis-serial-number__` dans l'URL.

**Le corps JSON**, aplati par `flatten()` en à la fois ses chemins pointés complets et ses
noms de feuilles nus. Les indices de tableau font partie du chemin mais pas du nom de feuille,
donc `network_interfaces.0.mac` est aussi atteignable par `mac` tout court.

### L'écart avec la règle d'origine

La règle initiale était que le corps de requête n'est jamais parsé comme du JSON. Elle a été
assouplie, délibérément et étroitement :

```rust
if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
    flatten(&value, &mut String::new(), &mut facts);
}
```

Non typé, opportuniste, et non fatal — un corps qui n'est pas du JSON apporte simplement la
botte de foin et rien de plus. Aucune struct n'est dérivée, donc aucune hypothèse sur le
schéma de Proxmox n'est gravée dans un type.

**C'est la forme « nom de feuille » qui justifie tout cela.** La documentation de Proxmox
avertit elle-même que le contenu de `dmi` « peut varier énormément selon le système ». Un
numéro de série ne peut être atteint d'aucune autre façon, parce que l'URL gravée dans une ISO
est la même pour toutes les machines. Un sélecteur disant *« un champ nommé `serial`, où qu'il
se trouve »* survit à une réorganisation qu'un chemin figé ne supporterait pas.

## Scoring

```rust
const IDENTITY_SCORE: u32 = 1_000;

fn score(control: &Control, identity: &[String], facts: &Facts) -> Option<u32> {
    if identity.iter().any(|n| !n.is_empty() && facts.haystack().contains(n)) {
        return Some(IDENTITY_SCORE);      // nommer une machine est le plus spécifique possible
    }
    if control.matchers.is_empty() { return None; }
    control.matchers.iter()
        .all(|(k, p)| facts.matches(k, p))
        .then_some(control.matchers.len() as u32)
}
```

- `identity` est le radical normalisé pour un document machine, et les `members` normalisés
  pour un groupe.
- **Tous** les critères doivent tenir ; le score est leur nombre.
- `IDENTITY_SCORE` vaut 1000 plutôt que `u32::MAX` pour que « une correspondance d'identité bat
  n'importe quel sélecteur » reste lisible, et qu'un sélecteur à mille critères reste un
  problème théorique plutôt qu'un problème subtil.

Les égalités se départagent sur le **nom trié**, le premier par ordre alphabétique :

```rust
.max_by(|(a, ca), (b, cb)| a.cmp(b).then_with(|| cb.id.cmp(&ca.id)))
```

La comparaison interne inversée est ce qui fait préférer à `max_by` le *plus petit* nom.
matchbox, l'antécédent le plus proche, documente que sa propre résolution entre groupes
concurrents « ne sera pas déterministe ». Celle-ci l'est, et un test l'épingle.

## Filtrage par format

```rust
fn wanted(facts: &Facts) -> Option<&'static [&'static str]>   // depuis les faits `segment`
fn acceptable(wanted: Option<&…>, format: &str) -> bool       // None ⇒ tout peut répondre
```

Le filtrage porte sur l'**extension**, jamais sur la famille. `.ks` et `.preseed` sont tous
deux `Kind::Text` ; filtrer par famille laisserait un preseed répondre à `/rhel/ks`.

`None` — une URL ne nommant aucun alias — ne contraint rien, ce qui garde `/answer` fonctionnel
pour un déploiement qui ne sert jamais qu'un format.

## Le cache du listing

```rust
struct Cached { version: Version, loaded_at: Instant, listing: Arc<Listing> }
```

Réutilisé seulement quand la `version()` du store est inchangée, **vaut `Some`**, et que moins
de `RELOAD_BACKSTOP` (1 s) s'est écoulé.

La lecture littérale de la spécification — relire le répertoire à chaque requête — c'est un
`readdir` plus un tri plus une passe de normalisation par requête. Avec un document de réponse
par machine, le débit s'effondre :

| Documents | Relecture littérale | Cache par mtime |
|---|---|---|
| 10 | 11 954 req/s | 12 922 req/s |
| 200 | 3 198 req/s | 12 890 req/s |
| 2 000 | 311 req/s | 12 520 req/s |
| 10 000 | — | 6 924 req/s |

Un `stat` remplace tout le parcours, et un nouveau document est quand même pris en compte sans
redémarrage — ce qui est la garantie que la spécification voulait réellement. Les radicaux
normalisés sont calculés une fois par lecture du store, pas une fois par requête.

> **Le filet n'est pas redondant.** Éditer le *contenu* d'un fichier de groupe ne bouge aucun
> mtime de répertoire, et un changement fait par un autre processus ne bouge aucun atomique en
> mémoire. Un test d'intégration couvre exactement cela.

Le coût restant à 10 000 documents est un balayage linéaire d'aiguilles précalculées — du CPU
pur, aucun appel système. Regrouper les aiguilles par longueur et faire glisser une fenêtre
sur le corps supprimerait ce coût, mais un déploiement de 10 000 machines se termine déjà en
moins de deux secondes. **Mesurez avant d'ajouter cela.**

## Construire un `Listing`

`build(snapshot)` fait tout ce qui est coûteux, une fois :

- parser chaque document, en gardant l'erreur plutôt qu'en faisant échouer le chargement ;
- normaliser chaque radical et chaque entrée de `members` ;
- résoudre les chaînes `extends`, en **détectant cycles et parents manquants** — le groupe
  cassé est écarté plutôt qu'appliqué à moitié, et le problème est enregistré ;
- pré-fusionner la chaîne de chaque groupe, et **la pré-rendre comme chaîne de caractères**
  quand elle ne porte aucun placeholder.

Ce dernier point est pourquoi le groupement est le chemin rapide : le cas courant en
datacenter ne parse rien par requête. `Group::has_placeholders` est le drapeau qui en décide.

`problems` est collecté ici, pas au moment de la requête, ce qui permet au
[garde-fou d'annulation](./admin.md#3-lécriture-qui-ne-peut-pas-casser-le-parc) de l'API
d'administration d'attraper un `extends` cassé avant que quiconque ne le demande.

## Résolution

`resolve()` est un match sur `(machine, machine_doc, group)` :

| Cas | Comportement |
|---|---|
| groupe seul | servir la chaîne préparée, ou cloner-remplir-nettoyer-rendre si templatisé |
| machine seule | remplir, nettoyer, rendre |
| les deux | chaîne de groupes, fusionner la machine par-dessus, remplir, nettoyer, rendre |
| aucun | retomber sur `default` pour le format demandé, qui peut lui-même `extends` un groupe |

Les variables de template sont les faits de la requête plus `machine` et `group`, que les
faits ne peuvent pas porter parce qu'ils ne sont connus qu'une fois la correspondance faite.

> **`machine` n'est lié que si un *document* machine a matché.** Une machine revendiquée par
> les `members` d'un groupe sans document à elle se résout avec `machine: None`, donc
> `{{ machine }}` dans un groupe échoue pour exactement les membres qu'il devait servir. Le
> [guide de templating](../guide/answers/templating.md#machine-exige-un-document-machine) dit
> d'utiliser un fait de requête à la place.
