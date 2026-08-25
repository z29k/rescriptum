---
title: Les contraintes
description: Des décisions de conception délibérées qui ressemblent à des choses à améliorer tant qu'on ne sait pas pourquoi elles sont là.
sidebar:
  label: Contraintes
  order: 2
---

# Les contraintes

Ce sont des décisions, pas des oublis. Plusieurs ressemblent, vues de l'extérieur, à des
améliorations évidentes. **N'en changez aucune sans demander** — et si vous en changez une,
changez cette page et `CLAUDE.md` avec.

## Asynchrone, sur tokio et hyper

La spécification d'origine demandait zéro dépendance et un thread par connexion. Les deux ont
été écartées délibérément, une fois que l'exigence est devenue *« absorber une rafale de
provisioning professionnelle »*. Un déploiement de 2 000 machines, ce sont 2 000 connexions
quasi simultanées, et un thread chacune fait 2 000 piles sur une machine à 512 Mo.

Ce qui a survécu de la spec : aucun `serde` derive, aucun framework, et une liste de
dépendances directes très courte. Voir [architecture](./architecture.md#dépendances).

## hyper directement, pas axum

axum ne donne aucun moyen de définir un **délai de lecture des en-têtes**, précisément le
garde-fou anti-slowloris qui a motivé le passage à l'asynchrone. Le routage ici est un seul
`if` sur la méthode et le chemin, donc un framework n'apporte rien et coûte la seule chose qui
comptait.

## Concurrence bornée, même si les tâches sont bon marché

Une connexion coûte des kilo-octets plutôt qu'un thread — c'est tout l'intérêt de la
réécriture asynchrone. Mais *bon marché* n'est pas *gratuit*, et un accept non borné
transforme quand même une rafale en épuisement mémoire.

Un `Semaphore` de `RESCRIPTUM_MAX_CONNECTIONS` plafonne les connexions en vol. Au-delà, le
serveur écrit un **`503` immédiat et ferme** plutôt que de mettre en file : un client à qui
on dit de réessayer s'en sort mieux qu'un client garé dans une file qui ne se videra pas.

## Le travail sur le système de fichiers passe par `spawn_blocking`

`read_dir` et `read` sont des appels bloquants, et bloquer un thread worker asynchrone bloque
toutes les autres connexions que ce thread pilote. **Sur un NAS dont le disque dort, ce n'est
pas théorique** — un réveil de disque se compte en secondes, pas en millisecondes.

`resolve()` porte à la fois le parsing et l'E/S, et n'est jamais appelé qu'à l'intérieur d'un
`spawn_blocking`. Un panic là renvoie un `500` ; il ne peut pas emporter le serveur.

## Ne jamais paniquer sur une entrée malformée

Tout échec de parsing devient une réponse d'erreur plus une ligne de log. Écrivez le code
comme s'il n'y avait pas de filet.

Il y en a un, délibérément : **le profil release ne définit pas `panic = "abort"`.** Avec le
déroulement, un panic est confiné à la connexion qui l'a causé au lieu de tuer un serveur en
pleine installation. Coût mesuré sur ARMv7 : **+2416 octets, +0,8 %**. Ne le « ré-optimisez »
pas.

Si la conception passe un jour à un pool de threads, ajoutez un `catch_unwind` à la frontière
du worker — un thread de pool qui meurt en silence est pire que l'un ou l'autre.

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
strip = true
# panic = "abort" est délibérément ABSENT
```

## Le store ne décide de rien

Il rend le texte brut des documents et un jeton de version bon marché. Correspondance,
chaînes `extends`, fusion, rendu et `check` vivent tous au-dessus et sont partagés.

**Gardez-le ainsi.** Dès qu'un backend se met à décider du comportement, les deux divergent —
et `tests/stores.rs` cesse de pouvoir prouver le contraire.

## L'organisation du stockage n'est pas l'URL

Répertoires et lignes de base sont un **espace de recherche** et doivent rester libres d'être
réorganisés. Une URL est un **contrat public gravé dans une ISO** et ne doit pas bouger parce
que quelqu'un a renommé un dossier. Une conception antérieure faisait du nom de répertoire *le*
segment d'URL et a été écartée pour exactement cette raison.

La conséquence est que la clé d'un document est **(identifiant, format)**, ce autour de quoi
le schéma SQLite est construit.

## Ne jamais construire un chemin de fichier à partir de données de requête

C'est le garde-fou contre la traversée de chemin, et il est structurel plutôt qu'une
vérification : seules les **entrées directes** du répertoire de réponses sont lues. Les
identifiants arrivant à l'API d'administration sont validés séparément, à la frontière de
l'API *et* dans les deux stores, parce qu'`export` les retransforme en noms de fichiers.

## Les réponses doivent être des documents valides

Avant la fusion, un fichier de réponse était servi comme des octets opaques, donc un fichier
malformé atteignait l'installateur. Maintenant c'est un `500` avec l'erreur de parsing dans le
log.

C'est le meilleur échec — un installateur qui reçoit du TOML à moitié valide échoue d'une
façon bien plus déroutante — mais **c'est** un changement de comportement, et des fixtures
écrites en pseudo-YAML ont cessé de fonctionner à ce moment-là.

## Échouer bruyamment

Un groupe manquant, un template impossible à remplir, un document qui ne parse pas : tous sont
des erreurs avec une raison, jamais une réponse au mieux.

Le raisonnement est toujours le même. **Une réponse à moitié construite installe une machine
de travers, et personne ne s'en aperçoit avant qu'elle ne tourne.** Une installation ratée se
remarque en quelques minutes.

## Asymétries délibérées

Deux endroits où la symétrie évidente est fausse exprès :

| | |
|---|---|
| **Le jeton de réponse n'est jamais limité en débit ; celui d'administration si** | une baie peut se trouver derrière une seule adresse, donc l'exclure transforme un mauvais jeton en déploiement raté. Aucun installateur ne parle à l'API d'administration |
| **Un jeton de réponse court avertit ; un jeton d'administration court empêche le démarrage** | refuser de démarrer laisserait un parc incapable de s'installer. Refuser de démarrer l'API d'administration ne coûte d'installation à personne |

## Ce que la spec demandait et n'a pas eu

`plans/rescriptum-spec.md` (dans le `.gitignore`, donc un contributeur ne l'aura pas) est le
compte rendu de ce qui a été demandé au départ, **pas une description de ce qui existe**. Le
projet l'a dépassée dans toutes les directions : multi-OS, sélecteurs, templating, une API
d'administration, un store en base.

Trois écarts précis, tous listés ci-dessus : asynchrone plutôt qu'un thread par connexion,
`panic = "abort"` omis, et le corps de requête parsé en JSON non typé pour en récolter des
faits. Là où la spec et cette page divergent, c'est cette page qui a raison.
