---
title: Tests
description: Où un test a sa place, la suite de conformité des deux stores, et les assertions qui ont réellement attrapé quelque chose.
sidebar:
  label: Tests
  order: 8
---

# Tests

734 tests. `cargo test` les fait tous tourner en une vingtaine de secondes — dont
l'essentiel dans `tests/tftp.rs`, qui attend de vrais délais UDP parce que c'est
précisément ce qu'il teste.

**`cargo test` ne lance pas les bancs qui comptent le plus** : le banc de démarrage, les
trois du paquet DSM, et la construction des chargeurs. Voir [Le paquet aussi est
testé](#le-paquet-aussi-est-testé-à-trois-endroits) et [le banc de
démarrage](#la-chaîne-de-démarrage-a-sa-place-dans-le-banc).

```bash
cargo test                                # tout
cargo test <nom>                          # un seul, par sous-chaîne
cargo test -- --nocapture                 # afficher stdout
cargo test --all-features                 # ce que lance la CI
```

## Où un test a sa place

| Suite | Cas | Pour |
|---|---|---|
| `tests/integration.rs` | 53 | le vrai binaire sur une vraie socket |
| `tests/cli.rs` | 89 | `render`, `check`, `import`, `export`, `config` et le fichier d'environnement — contre le vrai binaire |
| `tests/media.rs` | 45 | les médias de démarrage contre le vrai binaire, les deux listeners debout |
| `src/config.rs` | 56 | l'environnement, ce qui refuse de démarrer, et qui l'emporte du fichier ou de l'environnement |
| `tests/stores.rs` | 48 | **chaque comportement, contre les deux stores** |
| `tests/power.rs` | 12 | le client Redfish, contre un service factice auquel **le vrai curl** parle |
| `tests/tftp.rs` | 30 | le TFTP sur de l'UDP réel : les tours de parole, et ce qu'une liaison ratée ne doit pas coûter |
| `src/select.rs` | 29 | normalisation, scoring, superposition, remplissage de templates |
| `src/format/mod.rs` | 28 | parsing, fusion, clés de contrôle, alias d'endpoint |
| `src/tui.rs` | 13 | ce qu'une interface terminal retient, et quand elle a le droit de travailler — sans dessin |
| `src/tail.rs` | 12 | suivre le log : rotation, troncature, et la surface d'analyse |
| `src/edit.rs` | 8 | l'aller-retour $EDITOR, à travers le garde |
| `src/power.rs` | 9 | piloter un contrôleur, et ce qu'un hook tué rapporte |
| `src/guard.rs` | 5 | l'écriture gardée, et le rollback sur le store fichier |
| `src/controllers.rs` | 18 | the controllers file: what it refuses, and why each refusal exists |
| `src/redfish.rs` | 12 | curl's option file, the quoting, and reading a vendor's error |
| `tests/admin.rs` | 28 | l'API d'administration de bout en bout, formats compris |
| `src/envfile.rs` | 23 | le parseur et l'écrivain du fichier d'environnement, et ce que chacun refuse |
| `src/facts.rs` | 22 | parsing de query, aplatissement JSON, globbing |
| `src/format/xml.rs` | 18 | l'arbre XML — appariement, entités, fidélité |
| `src/merge.rs` | 11 | la fusion profonde TOML |
| `tests/guards.rs` | 7 | le jeton de réponse, et le verrouillage qui délibérément n'existe pas |
| `src/installed.rs` | 6 | une machine qui signale son installation, et ce qu'il ne faut jamais désarmer |
| `src/log.rs` | 15 | lecture des niveaux, et l'arithmétique d'horodatage |
| `src/boot/*.rs` | 128 | le lecteur ISO, le repérage, le catalogue, les sources d'images, les plans de patch, le menu, la table des chargeurs, les extraits DHCP, cpio et SHA-256 |
| `src/admin.rs`, `src/capture.rs`, `src/store/mod.rs` | 21 | comportement unitaire |

## `tests/common/mod.rs` — les fixtures que toutes les suites partagent

Les réponses sont stockées à raison d'**un répertoire par identité**, donc une fixture ne
peut plus être un simple nom de fichier. `seed()` prend le nom dans lequel un test pense —
`98fa9b50d810.toml`, `groups/rack-a.toml`, `default.toml` — et l'écrit **via `StoreWrite`**,
si bien qu'elle atterrit exactement là où une écriture de l'API d'administration la mettrait
et ne peut pas diverger de l'agencement. Un nom que le store refuserait (une extension que
personne ne sert) est écrit littéralement, parce que ces fixtures existent justement pour
prouver qu'un fichier égaré ne répond à rien.

Une seule copie, pas une par suite — le même raisonnement qui fait de `loaders.rs` une table
unique lue par TFTP *et* par l'extrait DHCP. Quatre copies d'une correspondance sont quatre
occasions qu'une fixture atterrisse là où le serveur ne regarde pas, et un test qui ne sème
rien passe pour la mauvaise raison.

## `tests/stores.rs` — la suite de conformité

Chaque cas de comportement tourne **deux fois**, une par store, et affirme le résultat
identique. Cette suite est ce qui empêche deux backends de diverger.

**Un nouveau comportement a sa place là, pas dans un test propre à un store.** Un test qui
couvre un seul backend prouve la moitié de ce qu'il prétend — et la moitié qu'il ne couvre pas
est exactement là où se cache une divergence.

## `tests/cli.rs` — les commandes qu'on dit aux gens de lancer

`check` est ce que [`deploy.sh`](./building.md#déployer-un-build) lance avant d'expédier
quoi que ce soit, et ce que la documentation dit de mettre en CI — donc **son code de
sortie est un contrat**, pas une commodité. La séparation stdout/stderr de `render` en est
un autre : le document part sur stdout pour que `render … > answer.toml` donne un fichier
utilisable, et la ligne de provenance part sur stderr pour qu'elle ne s'y retrouve pas.

Également épinglé ici : l'aller-retour `import` → `export` est **identique octet pour
octet**, commentaires et mise en forme compris. C'est ce qui rend la base sûre à adopter
*et* sûre à quitter ; si cela cesse d'être exact, `export` n'est plus une porte de sortie.

## `tests/integration.rs` — contre le vrai binaire

Il démarre le binaire réel sur un **port éphémère** et lui parle en HTTP. Le binaire affiche
l'adresse qu'il a bindée, donc il n'y a ni course sur le port ni « on dort et on espère ».

Cette suite existe parce que certains échecs sont invisibles aux tests unitaires. L'exemple le
plus clair : hyper **panique à l'exécution** si `header_read_timeout` est défini sans
`.timer(…)`. Ça compile. Seule une vraie connexion le trouve.

Explicitement couverts :

- une **requête tronquée**, et une **sans `Content-Length`** ;
- un `Content-Length` aberrant — *et* un corps chunked qui dépasse le plafond en cours de
  route, l'autre chemin d'entrée, où la limite saute en pleine lecture ;
- une méthode inconnue, un corps vide, un corps de 1 Mo, un corps qui n'est pas de
  l'UTF-8 valide ;
- le plafond de connexions : au-delà, un `503` immédiat plutôt qu'une file — et le permis
  qui revient ensuite ;
- **et, après chacun de ces cas, que le serveur répond encore.** C'est cette dernière
  assertion qui compte — la maltraitance n'a d'intérêt que si le serveur y survit.

> **`cargo test` ne reconstruit pas `target/debug/rescriptum`.** Une vérification manuelle
> contre un binaire périmé a un jour « reproduit » un bug déjà corrigé. Reconstruisez avant de
> triturer le binaire à la main.

## `tests/tftp.rs` — un transfert est une conversation

Rien ici ne se prouve depuis l'intérieur d'une fonction. Les blocs, les acquittements, la
retransmission, le paquet vide qui termine un transfert — chaque bug qui vaut d'être
attrapé vit dans les tours de parole, et la première exécution en a trouvé deux, du genre
« marche à la main, jamais après un redémarrage ». **Un fichier dont la longueur est un
multiple exact de la taille de bloc doit se terminer par un paquet de données vide** ;
sans lui le client attend éternellement un dernier bloc qui ne vient jamais.

Cette suite porte aussi le seul écouteur de ce serveur dont l'échec n'est *pas* fatal. Un
port TFTP qu'on ne peut pas lier ne doit pas emporter les réponses et les médias avec lui
— mesuré sur DSM, où la capacité est accordée hors du paquet et où une mise à jour la perd
— donc le test squatte le port, puis vérifie trois choses d'un coup : le serveur est monté,
il a averti en disant ce qui marche encore, et `boot check` sort toujours en non-zéro.

Cette dernière assertion est d'abord passée pour la mauvaise raison : trois chargeurs
manquants faisaient déjà échouer la commande. Le montage écrit maintenant tous les
chargeurs que la table nomme, et une exécution témoin avec le TFTP coupé prouve que le
répertoire est propre par ailleurs.

## `tests/media.rs` — les médias de démarrage contre le vrai binaire

Les deux listeners debout, et chaque cas d'abus se termine en prouvant que le serveur
répond toujours. Un cas prouve la propriété pour laquelle la socket séparée existe : **les
réponses continuent d'aboutir pendant que quatre transferts d'image sont en cours.**

Il n'y a délibérément **aucune ISO binaire dans ce dépôt**. `boot::iso::build` écrit des
images en mémoire, derrière la fonctionnalité `test-support`, pour qu'elle n'atteigne
jamais un binaire de release.

## La chaîne de démarrage a sa place dans le banc

`packaging/boot-rig/run.sh` n'est pas du Rust et `cargo test` ne le lance pas. Il démarre
une machine revendiquée et une non revendiquée dans QEMU sous TCG, sur un pont privé sans
lien montant, et vérifie quatre marqueurs : la passe DHCP a répondu depuis notre propre
extrait généré, un chargeur a été récupéré en TFTP, la machine non revendiquée est
retombée sur son disque local, et la machine revendiquée a atteint sa propre réponse. La
CI fait la même chose plus une casse délibérée.

**Un invité QEMU ponté dans un conteneur a une MAC à lui, et le commutateur virtuel de
Docker Desktop ne transmet pas les trames d'une MAC qu'il n'a pas attribuée** — mesuré,
d'où un banc principal en un seul conteneur plutôt qu'en quatre sur un réseau Docker.

## Vérifier qu'un test peut échouer

Un test qui passe pour la mauvaise raison est pire que pas de test : il annonce une
couverture qui n'existe pas. Avant de faire confiance à un nouveau test, cassez ce qu'il
garde et regardez-le rougir.

Un test de cette suite n'a pas survécu à cette vérification. Il prétendait protéger la
clause `version.is_some()` du cache du listing ; en la retirant, il restait vert — parce
qu'avec l'un comme l'autre store, une version n'est illisible que lorsque le store est aussi
vide, si bien que la clause ne peut pas se déclencher. Le test prouve quelque chose de réel
— un répertoire qui apparaît après le démarrage est servi dès la requête suivante — et le
dit maintenant.

## Assertions à copier

- **Affirmez sur des valeurs parsées, pas sur la mise en forme.** Remplacer une table par un
  scalaire laisse la décoration d'origine de la clé, donc la sortie peut se lire `value= 3` —
  du TOML valide, un texte différent. Une comparaison de chaînes échoue là pour la mauvaise
  raison, ou passe pour une mauvaise raison.
- **Les tests d'invalidation de cache doivent partager une seule instance d'`Answers`.** Un
  test qui en construit une nouvelle à chaque appel contourne complètement le cache et ne
  prouve silencieusement rien.
- **`Config::from_lookup` prend une closure**, pour que les tests de configuration ne touchent
  jamais l'environnement du processus — et ne se courent donc jamais après sous un runner
  parallèle.
- **Vérifiez que l'ancien texte a bien été trouvé avant d'écrire.** Deux patchs
  `python`/`sed` dans l'histoire de ce projet n'ont silencieusement rien matché et n'ont été
  attrapés qu'en vérifiant le nombre de tests ensuite.

## Les exemples de réponses sont aussi un test

```bash
RESCRIPTUM_ANSWERS_DIR=examples cargo run -- check
```

[`examples/`](https://github.com/z29k/rescriptum/tree/main/examples) contient un exemple
travaillé de chaque format, et c'est le seul endroit où ils sont montrés en train de se
composer ensemble. Deux d'entre eux ont attrapé de vrais bugs — un doctype manquant et un
attribut `pass` non apparié. Gardez-les fonctionnels.

## Le paquet aussi est testé, à trois endroits

`cargo test` ne touche pas au paquet DSM, parce que rien là-dedans n'est du Rust. Trois
harnais s'en chargent, et chacun prouve ce que les autres ne peuvent pas.

| | Prouve | Coût |
|---|---|---|
| [`packaging/dsm/check-spk.sh`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/check-spk.sh) | l'archive est structurellement ce que DSM attend — tar externe non compressé, les six champs d'`INFO`, une version tout en segments numériques, `os_min_ver` au moins 7.1, icônes 64×64 et 256×256, scripts exécutables sans CRLF, **le `--version` du binaire empaqueté**, et l'application de bureau : un `dsmappname` nommant une classe que son `ui/config` déclare vraiment, un nom de fichier JavaScript qui porte la version, et un backend qui vérifie toujours la session DSM et `administrators` | des secondes, **à chaque push** |
| [`packaging/dsm/lifecycle-test.sh`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/lifecycle-test.sh) | tout ce que les *scripts* du paquet décident, contre un faux arbre `/var/packages` : le fichier d'environnement écrit une fois et une seule, les valeurs de l'assistant **et leur absence**, le service qui survit à son propre script de démarrage et répond à `/health`, les codes de sortie que lit Package Center, une mise à jour qui ne doit pas toucher une configuration éditée à la main, une désinstallation qui ne doit pas toucher aux réponses — **et le backend de l'application de bureau**, piloté avec un authentificateur bouchonné : refuser l'absence de session, refuser un non-administrateur, refuser une écriture sans en-tête d'intention, refuser celle qui empêcherait le serveur de démarrer, et ne jamais livrer un jeton au navigateur | des secondes, **à chaque push** |
| [`packaging/dsm/vm/on-dsm.sh`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/vm/on-dsm.sh) | la machinerie propre à DSM — le worker `data-share` et son ACL, le worker `port-config`, l'unité systemd générée, logrotate contre un descripteur vivant, si Package Center accepte l'archive — **et qu'une machine qui demande sa configuration en reçoit une** : un POST avec le matériel dans le corps, auquel répond le fichier de cette machine fusionné par-dessus le groupe qui la revendique. Elle porte aussi **la seule route vers le port 69** et la capacité de ce NAS à atteindre l'index d'un éditeur : que `69/udp` survive dans l'entrée de pare-feu acquise, que le paquet réponde encore sans la capacité, et que `setcap cap_net_bind_service=+ep` puis un redémarrage lient `udp/69` sous le processus non privilégié du paquet | des minutes, sur une VM DSM 7 — puis sur le DS416j |

```bash
packaging/dsm/lifecycle-test.sh                     # le premier .spk de dist/ qui tourne ici
docker compose -f packaging/dsm/vm/docker-compose.yml up -d   # une machine DSM 7.2
packaging/dsm/vm/on-dsm.sh admin@<hôte> -p 2222     # contre elle
packaging/dsm/vm/on-dsm.sh admin@nas                # le verdict
```

La VM, c'est `vdsm/virtual-dsm`, qui installe la Virtual DSM officielle de Synology — aucune
image de loader à trouver. KVM la rend rapide, pas possible : sans `/dev/kvm` elle émule, dix
fois plus lentement, et c'est à ça que sert `docker-compose.emulated.yml`. En revanche elle
veut **14 Gio libres** pour son stockage, en dur dans l'image.

Le dernier est **destructeur exprès** — il met à jour par-dessus un fichier d'environnement
édité à la main et un canary dans le dossier partagé, puis désinstalle, puis vérifie que les
deux ont survécu. Ces deux gardes sont les choses les plus coûteuses à rater dans ce paquet,
et le premier `.spk` publié est celui dont les scripts de désinstallation tourneront pendant
la première mise à jour de tout le monde.
[`packaging/dsm/vm/README.md`](https://github.com/z29k/rescriptum/blob/main/packaging/dsm/vm/README.md)
décrit le banc d'essai : ce dont il est une preuve, et ce dont il ne l'est pas.

La même règle que partout ailleurs vaut pour eux : **cassez ce qu'ils gardent et
regardez-les virer au rouge.** Annuler la garde de `postinst` à la mise à jour, faire
supprimer le partage par `postuninst`, renvoyer `1` pour un paquet arrêté et refuser
`prestart` transforme 33 vérifications vertes en 25 vertes et 8 rouges — c'est ainsi qu'on
sait que le harnais teste quelque chose. Aujourd'hui c'est **85** vérifications dans
`lifecycle-test.sh`, **28** dans `check-spk.sh` et **52** sur la machine ; les trois dernières
ajoutées ont chacune été vues rouges de la même façon — en remettant
`RESCRIPTUM_TFTP_ADDR=off`, en supprimant le rapport du panneau sur l'état du TFTP, et en
lui faisant prétendre qu'il livre alors que rien n'est lié.

## CI

`.github/workflows/ci.yml`, à chaque push sur `main` et `develop` et à chaque pull request :

| Job | Lance |
|---|---|
| **gates** | `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo test --all-features`, `cargo build --release --no-default-features` |
| **docs** | construit le site public et lance `notabene lint` |
| **audit** | `cargo audit --deny warnings` sur l'arbre de dépendances |
| **cross** | un build ARMv7-musl complet, puis affirme que le binaire est bien `statically linked`, puis assemble les deux `.spk`, les contrôle structurellement et déroule le cycle de vie du paquet |

Le job cross n'est pas redondant. **SQLite est compilé depuis les sources, et `armv7-musl` est
la cible la moins indulgente qui soit livrée** — c'est là qu'une dépendance C casse en premier.
L'attraper sur un push vaut mieux que l'attraper en train de faire une release.

Le job **audit** est l'autre moitié de la règle « ajouter une dépendance exige une raison » :
une raison de l'ajouter n'est pas une raison de la garder. `--deny warnings` échoue aussi sur
un crate non maintenu ou yanké, pas seulement sur une vulnérabilité. Quand quelque chose
apparaît sans correctif, ajoutez `--ignore RUSTSEC-…` avec une ligne expliquant pourquoi,
plutôt que de retirer le drapeau.

Chaque action utilisée est une action officielle `actions/*`, et Zig comme `cargo-audit` sont
installés directement plutôt que via une action tierce. C'est délibéré : cette toolchain
vérifie et lie un binaire que des gens font tourner en root.

Le site de documentation a son propre garde-fou — voir
[le site de documentation](./docs-site.md#le-garde-fou-ci).
