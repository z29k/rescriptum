---
title: Servir sa première réponse
description: D'un répertoire vide à une machine recevant un document composé pour elle — cinq minutes, sans ISO.
sidebar:
  label: Première réponse
  order: 2
---

# Servir sa première réponse

Cinq minutes, un terminal, aucun installateur nécessaire. Tout ici est testable hors ligne :
`rescriptum render` résout une réponse exactement comme le ferait le serveur, donc vous
pouvez avoir la bonne réponse avant qu'aucune machine ne démarre.

## 1. Un répertoire et un document

```console
$ mkdir -p answers/groups/rack-a
```

**Un répertoire par identité.** Un répertoire à la racine est une machine, nommé d'après
elle ; `groups/` contient ceux qui sont partagés. Dans l'un comme dans l'autre, l'extension
nomme le format et le reste du nom de fichier n'est qu'une étiquette. Commencez par un
groupe, puisque c'est la forme que prend presque tout déploiement réel — une baie de
machines d'accord sur tout sauf sur les disques qu'elles ont :

```toml
# answers/groups/rack-a/proxmox.toml
members = ["98:fa:9b:50:d8:10", "98:fa:9b:50:d8:11"]

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
root-password-hashed = "$6$rounds=656000$REPLACE$ME"

[network]
source = "from-dhcp"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid1"
disk-list = ["sda", "sdb"]
```

`members` liste les machines pour lesquelles ce groupe répond. Le style de séparateur n'a
pas d'importance — `98:fa:9b:50:d8:10`, `98-FA-9B-50-D8-10` et `98fa9b50d810` sont une seule
MAC, des deux côtés de la comparaison. `members` est une clé de rescriptum, pas de Proxmox,
et elle est retirée de ce que l'installateur reçoit.

## 2. Voir ce qu'une machine recevrait

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum render 98:fa:9b:50:d8:11
# format=toml group=rack-a

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
root-password-hashed = "$6$rounds=656000$REPLACE$ME"

[network]
source = "from-dhcp"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid1"
disk-list = ["sda", "sdb"]
```

La première ligne part sur stderr et dit comment la réponse a été obtenue — la famille de
format, quel document machine a matché, quel groupe s'est appliqué. Le document lui-même
part sur stdout, donc `render … > answer.toml` ne vous donne que le document.

## 3. Une machine qui diffère

Le second nœud de la baie a quatre disques. Il reçoit un répertoire nommé d'après sa MAC,
contenant un document avec **seulement la différence** :

```toml
# answers/98-fa-9b-50-d8-10/proxmox.toml
[global]
fqdn = "node01.example.com"

[disk-setup]
zfs.raid = "raid10"
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum render 98:fa:9b:50:d8:10
# format=toml machine=98-fa-9b-50-d8-10 group=rack-a

[global]
keyboard = "fr"
country = "fr"
timezone = "Europe/Paris"
root-password-hashed = "$6$rounds=656000$REPLACE$ME"
fqdn = "node01.example.com"

[network]
source = "from-dhcp"

[disk-setup]
filesystem = "zfs"
zfs.raid = "raid10"
disk-list = ["sda", "sdb", "sdc", "sdd"]
```

Le groupe d'abord, le document propre à la machine par-dessus, et **la machine a gagné** partout où les
deux étaient en désaccord. Les tables ont fusionné clé par clé ; `disk-list` a été
**remplacée**, pas concaténée — une liste qui ne pourrait que grandir ne pourrait jamais
être raccourcie depuis une couche supérieure.

## 4. Vérifier l'ensemble

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum check
checking files:answers
  1 group(s), 1 machine document(s)
  note: toml answers not schema-checked — proxmox-auto-install-assistant is not on PATH
  ok — everything renders
```

`check` rend chaque machine et chaque membre de groupe, et signale tout ce qui casse : un
document qui ne parse pas, un groupe qui en étend un inexistant, un placeholder que rien ne
peut remplir. Là où le validateur de l'installateur est dans le PATH, il le lance aussi, et
dit quels formats il n'a pas pu vérifier.

C'est la commande à mettre en CI si vos réponses vivent dans git.

## 5. Les servir pour de vrai

```console
$ RESCRIPTUM_ANSWERS_DIR=answers rescriptum
2026-08-24T08:43:36Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:answers workers=10 max_conn=2048 timeout=10s
```

Dans un autre terminal, imitez ce qu'envoie l'installateur Proxmox :

```console
$ curl -s -X POST http://localhost:8000/answer \
    -d '{"network_interfaces":[{"mac":"98:fa:9b:50:d8:10","link":"up"}]}'
```

et regardez le serveur dire ce qu'il a fait :

```
2026-08-24T08:43:37Z 127.0.0.1:61721 POST /answer body=102 200 format=toml machine=98fa9b50d810 group=rack-a bytes=431
```

Cette ligne est tout le diagnostic disponible quand un déploiement dérape : qui a demandé,
quelle taille faisait son corps, ce qu'il a reçu, et à partir de quoi c'était construit.

Les nouveaux documents sont pris en compte au fur et à mesure — pas de redémarrage, pas de
signal de rechargement. L'apparition ou la disparition du répertoire entier d'une machine
est vue immédiatement ; un document ajouté ou modifié *à l'intérieur* de l'un d'eux est pris
en compte en moins d'une seconde.

## À lire ensuite

- **[Comment une réponse est choisie](./answers/selection.md)** — vous avez vu le nommage et
  l'appartenance ; les sélecteurs revendiquent une machine pour ce qu'elle *est*.
- **[Un document par système d'exploitation](./answers/formats.md)** — la même machine en
  Proxmox, en Debian, en Ubuntu, côte à côte.
- **[Templating](./answers/templating.md)** — `fqdn = "node-{{ serial }}.example.com"`, pour
  qu'un groupe couvre une baie sans un répertoire par machine.
- **[Préparer les médias d'installation](./iso.md)** — l'URL à graver dans l'ISO, par OS.
