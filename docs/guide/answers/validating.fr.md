---
title: Valider ce qui sera servi
description: Une réponse fusionnée est un document que personne n'a jamais écrit. render l'affiche ; check rend tout et signale ce qui casse.
sidebar:
  label: Validation
  order: 5
---

# Valider ce qui sera servi

Avant que les réponses ne se composent, un administrateur écrivait un document complet et le
validait :

```console
$ proxmox-auto-install-assistant validate-answer answer.toml
```

Une fois qu'une réponse est assemblée à partir d'une chaîne de groupes, plus un document
machine, plus un remplissage de template, **le document que reçoit l'installateur est un
document que personne n'a jamais vu** — et une mauvaise fusion se manifeste par une
installation automatisée ratée à 3 h du matin. Deux sous-commandes existent pour combler ce
manque, et tout changement de la fusion doit les garder fonctionnelles.

## `render` — ce que cette machine recevrait

```console
$ rescriptum render 98:fa:9b:50:d8:10                              # par identité
$ rescriptum render --query "serial=7ABC123&mac=98:fa:9b:50:d8:10" # par étiquette
$ rescriptum render --query "path=/rhel/ks&serial=7ABC123"         # endpoint compris
$ rescriptum render --body captured-request.json                   # un vrai corps capturé
```

Il résout exactement comme le serveur — même correspondance, même superposition, même
remplissage de template — et affiche le résultat. Le **document part sur stdout** ; la ligne
expliquant comment il a été obtenu part sur **stderr** :

```console
$ rescriptum render 98:fa:9b:50:d8:10
# format=toml machine=98-fa-9b-50-d8-10 group=rack-a
[global]
…
```

donc une redirection ne vous donne que le document :

```console
$ rescriptum render 98:fa:9b:50:d8:10 > /tmp/answer.toml
```

Ajoutez `path=…` à `--query` quand vous voulez vérifier ce qu'un *endpoint particulier*
répondrait — sans cela, la résolution n'est pas contrainte par le format et peut choisir un
document que la vraie URL aurait exclu.

Le code de sortie est 0 quand quelque chose s'est résolu, non nul quand rien ne s'appliquait
(le serveur aurait renvoyé un 404) ou quand le rendu a échoué.

## `check` — tout rendre, signaler ce qui casse

```console
$ rescriptum check
checking files:examples
  10 group(s), 8 machine document(s)
  group "rhel-compute" selects on serial=7ABC*
    (verify with: rescriptum render --query "...")
  group "ubuntu-web" selects on file=user-data product=PowerEdge R6*
    (verify with: rescriptum render --query "...")
  1 answer(s) validated by their installer's own tool
  note: no schema validator exists for preseed answers
  note: toml answers not schema-checked — proxmox-auto-install-assistant is not on PATH
  ok — everything renders

Well-formed and merging cleanly is not the same as valid for an
installer. Where a validator exists and is installed it was used above;
install proxmox-auto-install-assistant, xmllint or ksvalidator for the rest.
```

Ce qu'il fait :

- **Signale les problèmes de chargement** — un groupe qui en étend un inexistant, un cycle
  entre groupes, un document qui ne parse pas.
- **Rend chaque document machine**, et chaque membre de chaque groupe. C'est ce qui exerce
  réellement la fusion.
- **Nomme les groupes qui sélectionnent sur un bloc `match`** et dit qu'il n'a pas pu les
  essayer, plutôt que de laisser croire qu'ils ont été vérifiés — un sélecteur a besoin d'une
  vraie requête.
- **Signale un groupe sans `members` ni `match`** comme atteignable seulement via `extends`,
  au cas où ce ne serait pas l'intention.
- **Appelle le validateur de l'installateur** là où il existe et est dans le PATH, et dit
  quels formats il n'a pas pu vérifier.

Le code de sortie est 0 quand tout se rend, 1 dès que quelque chose a échoué — il tombe donc
directement dans une CI.

### Les validateurs qu'il connaît

| Format | Outil |
|---|---|
| `toml` | `proxmox-auto-install-assistant validate-answer` |
| `xml`, `autoyast`, `unattend` | `xmllint --noout` |
| `ks` | `ksvalidator` |
| `yaml`, `json`, `ign`, `preseed`, `cfg`, `ipxe` | aucun n'existe — rendez et lisez |

Un outil manquant est signalé une fois comme note, jamais traité comme un échec. Un
vérificateur qui refuse de tourner sans outillage optionnel est un vérificateur que personne
ne lance.

`check` n'est pas lui-même un vérificateur de schéma : il prouve que vos documents sont bien
formés et fusionnent proprement. Pour tout ce dont il ne peut pas appeler un validateur,
faites-le vous-même :

```console
$ rescriptum render 98:fa:9b:50:d8:10 > /tmp/answer.toml
$ proxmox-auto-install-assistant validate-answer /tmp/answer.toml
```

### Ce que `check` ne peut pas prouver

`check` rend chaque machine à partir de sa **seule identité**. Il n'a pas de requête, donc il
ne peut pas fournir de faits qui n'arrivent qu'avec une requête — un `serial` depuis un corps
POSTé, une `mac` depuis une query string. Un template en ayant besoin est signalé comme
problème :

```
FAIL group "rack-a" member "98fa9b50d811": template needs {{ serial }}, but this request carries no "serial"
```

C'est exact — `check` ne peut réellement pas prouver que cette réponse se rend — mais cela
signifie qu'un jeu templatisant délibérément sur des faits de requête ne reviendra pas
propre. Vérifiez ceux-là avec `render --query` et des faits représentatifs. Voir
[templating](./templating.md#check-et-les-faits-propres-à-la-requête).

## En CI

Si vos réponses vivent dans git, cela mérite un job à part :

```yaml
# .github/workflows/answers.yml
name: answers
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Get rescriptum
        run: |
          curl -fsSL https://github.com/z29k/rescriptum/releases/latest/download/rescriptum-x86_64-unknown-linux-musl.tar.gz \
            | tar xz --strip-components=1
      - run: RESCRIPTUM_ANSWERS_DIR=answers ./rescriptum check
```

Ajoutez `proxmox-auto-install-assistant` au runner et le même job vérifie aussi le schéma du
TOML.

## Avant de déployer

[`deploy.sh`](../operations/deployment.md#remplacer-une-instance-en-cours) lance `check` avant
d'expédier quoi que ce soit, et refuse de déployer si les réponses ne reviennent pas propres.
Servir un jeu de réponses cassé est pire que ne pas déployer.
