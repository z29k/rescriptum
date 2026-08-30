---
title: Servir les médias de démarrage
description: Servir l'installeur lui-même — noyau, initrd et image — depuis le serveur qui décide déjà la réponse, sur son propre listener.
sidebar:
  label: Médias de démarrage
  order: 8
---

# Servir les médias de démarrage

Une réponse dit à une machine *comment* s'installer. Elle ne dit rien de l'endroit d'où
vient l'installeur — et jusqu'ici c'était le serveur web de quelqu'un d'autre, hébergeant
des images que personne ne confrontait aux réponses écrites pour elles.

Avec un répertoire de médias, le même serveur fait les deux. **La MAC d'une machine
choisit sa réponse *et* l'image pour laquelle cette réponse a été écrite, et les deux ne
peuvent plus diverger puisqu'un seul composant décide des deux.**

```console
$ export RESCRIPTUM_MEDIA_DIR=/srv/media
```

Non défini, tout est éteint. Rien ne change pour un déploiement existant tant que vous ne
la définissez pas.

## Où vivent les images de base

**Aucune image d'installation n'est dans ce projet, ni dans une version publiée.** Une
ISO est l'artefact de quelqu'un d'autre, elle pèse un à quatre gigaoctets, et elle change
à son propre rythme — trois raisons distinctes pour qu'elle vive sur votre disque plutôt
que dans le nôtre. `RESCRIPTUM_MEDIA_DIR` est l'endroit où vous les gardez, et **ce
répertoire est l'archive** : ce que l'éditeur a publié, sur disque, jamais modifié
ensuite.

Ce dernier point est une propriété, pas une promesse. Rien ici ne réécrit une image —
en préparer une produit un fichier compagnon et une injection appliquée *au fil de
l'eau* (voir [Préparer une image Proxmox](#préparer-une-image-proxmox)), de sorte que les
octets sur disque restent exactement ce que l'éditeur a publié et que leur empreinte
reste vérifiable contre le `SHA256SUMS` de l'éditeur. `media list` dit quelles entrées
sont l'archive et lesquelles en dérivent.

## Faire entrer une image

Trois façons, et la première est celle à privilégier.

### Choisir dans un catalogue

```console
$ rescriptum media sources
SOURCE       NAME              WHAT IT INSTALLS
proxmox-ve   Proxmox VE        the founding case — answers come from a file injected into the image
debian       Debian            netinst images; the answer is a preseed on the kernel command line
ubuntu       Ubuntu LTS        autoinstall, via a cloud-init datasource on the kernel command line
almalinux    AlmaLinux 9       kickstart, named on the kernel command line
rocky        Rocky Linux 9     kickstart, named on the kernel command line

$ rescriptum media sources proxmox-ve
reading https://enterprise.proxmox.com/iso/SHA256SUMS …
Proxmox VE — the founding case — answers come from a file injected into the image
  proxmox-ve_9.2-1.iso
  proxmox-ve_9.2-1-arm64.iso
  proxmox-ve_9.1-1.iso

$ rescriptum media add --from proxmox-ve proxmox-ve_9.2-1.iso
```

**Rien concernant une image précise n'est stocké dans ce serveur.** Chaque catalogue nomme
l'index de sommes que l'éditeur publie déjà à côté de ses propres images, et les noms comme
les empreintes en sont lus au moment où vous demandez — la liste est donc celle que cet
éditeur a aujourd'hui, et l'empreinte est la sienne. Une table d'URL figée dans une version
publiée proposerait les images du trimestre dernier, dont certaines supprimées depuis.

**Ce que cela vaut, dit franchement.** Prendre l'empreinte sur le serveur qui sert l'image
n'est *pas* une vérification de signature. En HTTPS cela authentifie le domaine de
l'éditeur et cela attrape un téléchargement tronqué, un miroir corrompu et un fichier qui a
changé en dessous — l'essentiel de ce qui arrive vraiment — et rien de plus. Si vous voulez
davantage, utilisez la section suivante avec une empreinte que vous avez obtenue vous-même.

### Laisser le serveur la récupérer

```console
$ rescriptum media add https://enterprise.proxmox.com/iso/proxmox-ve_8.4-1.iso \
    --sha256 9f86d081884c7d65…
fetching https://enterprise.proxmox.com/iso/proxmox-ve_8.4-1.iso
  with curl, into /srv/media/proxmox-ve_8.4-1.iso.part
######################################################################## 100.0%
verifying 1.5G …
fetched 1.5G via curl, digest verified
```

Elle atterrit sous un nom en `.part` et n'est renommée qu'une fois l'empreinte
vérifiée : **un téléchargement partiel ne devient jamais une entrée du catalogue** — le
catalogue analyse ce qu'il trouve, et une ISO tronquée s'analyse comme une image inconnue
qu'une machine essaierait ensuite de démarrer. Une récupération interrompue laisse le
`.part` en place, et relancer la commande la reprend.

`--sha256` est **obligatoire** ici, parce que rien d'autre ne vérifierait ce qui est
arrivé. Les éditeurs publient un `SHA256SUMS` à côté de l'image. Si vous voulez vraiment
vous en passer, dites `--unverified` — l'important est que sauter cette vérification soit
un acte délibéré et non le défaut, puisque cela décide ce que chaque machine du réseau
installe.

`--as NOM.iso` choisit le nom de fichier quand l'URL n'en implique pas d'utilisable.

::: tip Il n'y a pas de TLS dans ce binaire
rustls et un magasin de racines, c'est une quarantaine de crates et plus d'un mégaoctet
sur ARMv7, pour un travail dont chaque hôte a déjà l'outil. Donc ceci lance `curl`, ou
`wget` si c'est lui qui est installé, et le dit franchement s'il n'en trouve aucun — auquel
cas la réponse est celle ci-dessous.
:::

### Ou la poser vous-même

En SMB, en `scp`, depuis là où l'ISO se trouve déjà — le geste naturel sur un NAS — puis
l'enregistrer :

```console
$ rescriptum media add /srv/media/pve-8.4.iso --sha256 9f86d081884c7d65…
hashing /srv/media/pve-8.4.iso …
  10% (152.0M of 1.5G)
  …
pve-8.4  9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08
  proxmox Proxmox Virtual Environment 8.4-1
  kernel /boot/linux26
  initrd /boot/initrd.img
  wrote /srv/media/pve-8.4.media
```

`--sha256` est facultatif et mérite d'être fourni : une empreinte qui ne correspond pas,
c'est soit un téléchargement tronqué soit le mauvais fichier, et les deux installeraient
la mauvaise chose sur chaque machine qui demande. Rien n'est enregistré en cas d'écart.

**Rien n'est copié et l'image n'est jamais modifiée.** Ce que `media add` écrit, c'est le
fichier compagnon `.media` posé à côté, qui retient l'empreinte et ce que la détection a
trouvé. C'est tout l'intérêt : hacher 1,5 Go prend près d'une minute, et le serveur ne
doit jamais passer une minute *dans* une requête.

Une image sans compagnon apparaît quand même et est servie quand même — elle n'a
simplement pas d'empreinte à revérifier, et elle est analysée à la volée.

## Ce qu'il sait dire d'une image

```console
$ rescriptum media list
ID                   FAMILY   ARCH       VERSION                          SIZE  PINNED
pve-8.4              proxmox  x86_64     Proxmox Virtual Environment…     1.5G  9f86d0818
ubuntu-24.04         ubuntu   x86_64     Ubuntu-Server 24.04.1 LTS        2.1G  —
gparted-1.6          unknown  —          GPARTED-LIVE                   420.0M  —
```

Six familles sont reconnues — Proxmox, Debian, Ubuntu, RHEL et ses dérivés, SUSE et
Fedora CoreOS — à partir d'une table de marqueurs situés dans l'image. Là où un éditeur a
laissé une chaîne de version, elle est reprise ; l'identifiant de volume sert de repli.

**Une image que rien ne reconnaît est quand même listée et quand même servie.** Ne pas
savoir la décrire n'est pas la même chose que ne pas savoir s'en servir : elle peut être
`sanboot`ée, écrite sur une clé, ou récupérée entière par le firmware. Ce qu'elle ne peut
pas faire, c'est produire une strophe de démarrage, et le serveur le dit plutôt que de
deviner.

## Les points d'entrée

Le listener média a sa propre socket, sur `0.0.0.0:8001` par défaut.

| Route | Ce qui revient |
|---|---|
| `GET /` | le catalogue en texte, ou en JSON avec `Accept: application/json` |
| `GET /<id>/iso` | l'image |
| `GET /<id>/kernel` | le noyau, diffusé **depuis l'intérieur** de l'image |
| `GET /<id>/initrd` | l'initrd, de même |
| `GET /<id>/initrd+iso` | l'initrd avec l'image ajoutée, pour les vieux chargeurs |
| `GET /<id>/file/<chemin>` | n'importe quel fichier dans l'image |
| `GET /health` | `200 OK` |

Rien n'est extrait et rien n'est décompressé. Un fichier dans une image ISO9660 est une
plage d'octets contiguë : servir `/pve-8.4/kernel` est donc un positionnement et une
longueur — le même travail de quelques kilo-octets que l'image fasse 400 Mo ou 4 Go.

Les plages (`Range`), `ETag`, `If-Range` et `HEAD` sont tous traités, parce que les vrais
clients en ont besoin : casper d'Ubuntu et anaconda de Red Hat récupèrent tous deux par
plages, et le démarrage HTTP UEFI envoie un `HEAD` avant de récupérer quoi que ce soit.

### Pourquoi c'est un second listener

Ce n'est pas une préférence — trois raisons distinctes, dont une seule suffirait :

- Le point de réponse répond sur **n'importe quel chemin**, puisque l'URL est gravée dans
  une ISO. Un préfixe `/media/…` découperait un espace réservé dans un espace
  délibérément ouvert.
- `RESCRIPTUM_TIMEOUT_SECS` est une échéance de connexion entière de dix secondes. Un
  transfert de 1,5 Go dure quinze secondes en gigabit et deux minutes en 100 Mbit : tous
  les téléchargements seraient tués en vol — et cela ressemblerait à un réseau instable,
  pas à un réglage.
- Un téléchargement retient un jeton de connexion pendant des minutes. Partager ce budget
  avec les réponses, c'est un déploiement qui affame ses propres installations.

Les deux ont des budgets séparés, et un test le prouve au lieu de l'espérer : les
réponses continuent d'aboutir avec quatre transferts en cours.

## Démarrer une machine depuis tout ça

`media ipxe` écrit la strophe de démarrage d'une image :

```console
$ rescriptum media ipxe pve-8.4
#!ipxe
# Proxmox Virtual Environment 8.4-1 — generated by `rescriptum media ipxe pve-8.4`.
# An ordinary answer document: selection, layering and templating all apply.
kernel http://192.0.2.10:8001/pve-8.4/kernel ramdisk_size=16777216 rw quiet initrd=initrd.img \
    splash=silent proxmox-start-auto-installer
initrd http://192.0.2.10:8001/pve-8.4/initrd initrd.img
initrd http://192.0.2.10:8001/pve-8.4/iso proxmox.iso
boot
```

**Il imprime un script, il n'en installe pas.** Enregistrez-le dans le répertoire des
réponses et c'est un document de réponse ordinaire — sélectionné, superposé et
gabarisé comme n'importe quel autre :

```console
$ rescriptum media ipxe pve-8.4 > /srv/answers/groups/rack-a/boot.ipxe
```

C'est bien le point. Le serveur ne devient pas malin sur le démarrage ; il gagne un
générateur, et le moteur de composition que vous avez déjà fait le reste. Un `{{ mac }}`
dans l'URL de réponse générée est rempli à chaque requête depuis les faits de la machine.

Chaque famille reçoit ce dont elle a réellement besoin, et elles ne se ressemblent pas :

| Famille | Comment la réponse lui parvient |
|---|---|
| Proxmox VE | dans l'image, via `auto-installer-mode.toml` — et `proxmox-start-auto-installer` sur la ligne de commande pour choisir la voie automatisée |
| Debian | `preseed/url=…` |
| Ubuntu | `ds=nocloud-net;s=…/`, d'où cloud-init récupère `user-data` *et* `meta-data` |
| Famille RHEL | `inst.ks=…` |
| SUSE | `autoyast=…` |
| Fedora CoreOS | `ignition.config.url=…` |

Proxmox est le cas à part, et il vaut la peine de savoir pourquoi : c'est le seul qui
porte l'emplacement de la réponse *à l'intérieur de l'image* plutôt que sur la ligne de
commande du noyau. C'est aussi pour cela que c'est le seul à devoir passer une fois par
`prepare-iso` — voir [Préparer les médias d'installation](../iso.md).

::: tip Vous avez déjà lancé `prepare-iso --pxe` ?
Cela laisse un répertoire contenant `vmlinuz`, `initrd.img` et une ISO allégée. Pointez
`RESCRIPTUM_MEDIA_DIR` dessus et cela fonctionne tel quel : l'image allégée est toujours
reconnue comme Proxmox, et le noyau et l'initrd posés à côté sont trouvés et servis.
:::

## Préparer une image Proxmox

Proxmox est la seule famille à porter l'*emplacement* de la réponse à l'intérieur de
l'image, dans `/auto-installer-mode.toml`. Cela imposait jusqu'ici de lancer
`proxmox-auto-install-assistant prepare-iso` ailleurs d'abord.

```console
$ rescriptum media prepare pve-8.4
pve-8.4-http  prepared from pve-8.4
  answer   http://192.0.2.10:8000/proxmox
  injects  /auto-installer-mode.toml (198 bytes)
  image    1610612736 bytes (source 1610610688 + 2048 appended)
  wrote    /srv/media/pve-8.4-http.media

Nothing was copied. Serve it as /pve-8.4-http/iso, or write it to a stick with
  rescriptum media export pve-8.4-http /tmp/pve-8.4-http.iso
```

**Ce qui vient d'être écrit est un fichier compagnon : environ deux cents octets qui
tiennent lieu de 1,5 Go.** La source n'est jamais modifiée, jamais copiée, et son
empreinte publiée reste vérifiable. Le fichier est injecté *au fil de l'eau*, donc
changer plus tard l'URL de réponse réécrit ces deux cents octets plutôt qu'un gigaoctet —
et les deux entrées apparaissent dans `media list`, adossées à une seule image sur disque.

`--as NOM` choisit le nom de l'entrée dérivée, et `--url`, `--cert-fingerprint` et
`--token` disent ce qui va dans le fichier.

### Pour une clé USB

```console
$ rescriptum media export pve-8.4-http /tmp/pve-auto.iso
```

Matérialise exactement ce que le listener aurait servi, par le même chemin de code. Une
clé écrite autrement serait une seconde implémentation à maintenir honnête, et l'écart ne
se verrait que sur le bureau de quelqu'un.

### Quand il refuse

Refuser est ici une réponse **complète**, parce que le repli tient en une commande sur
n'importe quelle Debian et que ce serveur sert très bien son résultat :

```console
$ proxmox-auto-install-assistant prepare-iso pve.iso --fetch-from http --url …
```

Il refuse quand l'image n'a **ni Rock Ridge ni Joliet** — le fichier ne pourrait alors
exister que sous un nom 8.3 tronqué comme `AUTO_INS.TOM;1`, et l'installeur ne le
trouverait jamais. Il refuse une image **UDF**, parce qu'une ISO Windows ne garde ses gros
fichiers que dans l'arbre UDF et que patcher l'arbre ISO9660 produirait quelque chose qui
a l'air juste et ne l'est pas. Et il refuse quand le répertoire racine n'a **pas de mou**
dans aucun de ses secteurs : déplacer l'extent entraînerait les tables de chemins, ce qui
n'est délibérément pas fait.

Il refuse aussi de préparer une image non-Proxmox, en nommant l'alternative : toutes les
autres familles prennent l'URL sur la ligne de commande du noyau, là où `media ipxe` la
met déjà.

### Si la source change en dessous

Les décalages d'injection sont calculés contre une image donnée. Une source qui aurait
changé serait patchée au mauvais endroit, produisant une image qui se monte et qui est
fausse — le fichier compagnon retient donc la taille de la source, et le catalogue refuse
quand elle ne correspond plus :

```
  problem: pve-8.4-http.media: pve-8.4 was 1610610688 bytes when this was prepared and
  is 1610612736 now. The injection offsets no longer apply — re-run `media prepare`.
```

## Dire au serveur son propre nom

Dès qu'il écrit des URL dans les scripts qu'il sert, le serveur a besoin d'un nom pour
lui-même qu'une machine puisse réellement atteindre. `0.0.0.0:8001` n'en est pas un.

```console
$ export RESCRIPTUM_PUBLIC_HOST=192.0.2.10
```

**Un hôte, jamais une URL.** Pas de schéma, pas de port, pas de chemin — le serveur écrit
des URL pour deux listeners, et une valeur portant un port épinglerait chaque script
généré sur l'un d'eux. Chaque URL ajoute le port de son propre listener. Une valeur
portant l'un des trois est refusée au démarrage, en nommant lequel.

Laissée vide, elle demande à la table de routage laquelle des adresses de cet hôte fait
face à l'extérieur — et sur un segment sans route par défaut, se rabat sur la liste des
interfaces, ce qui sur un hôte à une seule adresse n'est pas une déduction du tout. Dans
les deux cas elle **dit au démarrage ce qu'elle a retenu**, et s'il y avait quelque chose
à trancher :

```
RESCRIPTUM_PUBLIC_HOST is not set — using 192.0.2.10, the only address this host has.
Every generated URL will name it.
```

```
warning: RESCRIPTUM_PUBLIC_HOST is not set — derived 192.0.2.10, which is what every
generated URL will name. This host also has 10.8.0.4. If the machines reach it on one of
those instead, set it explicitly.
```

C'est la seconde qu'il faut prendre au sérieux : une mauvaise déduction produit une machine
qui démarre, enchaîne, et se bloque sur une adresse qui n'existe pas. Nommer les autres
adresses est ce qui rend la question tranchable depuis le journal lui-même, plutôt qu'en
allant regarder l'hôte. Le NAT est le cas qu'aucune des deux lignes ne peut attraper :
l'adresse est bien celle de cet hôte, et bien celle que les machines n'atteignent pas.

## Le garder honnête

```console
$ rescriptum media check
checking media in /srv/media
  2 image(s), 1 verified against a recorded digest
  note: ubuntu-24.04 has no recorded digest — `media add` records one
  ok — everything recorded still matches
```

Son code de sortie est un contrat, comme celui de `check` : zéro quand tout ce qui a été
enregistré correspond toujours, un quand quelque chose a dérivé. `deploy.sh` s'y fie.

Une image qui a changé sous une empreinte enregistrée est la seule panne qui installe
silencieusement quelque chose que personne n'a relu, donc elle est bruyante :

```
  FAIL pve-8.4: the image no longer matches what was recorded
       recorded 9f86d081884c7d65…
       found    7d793037a0760186…
```

Ce que cela prouve, c'est **l'intégrité, pas l'authenticité** : ce qui est servi est ce
qui a été enregistré. Savoir si ce qui a été enregistré est bien ce que l'éditeur a
publié relève de ses propres signatures, et `--sha256` au moment du `media add` est
l'endroit où cette vérification se place.

## Qui a le droit de récupérer

Le trafic de démarrage n'est pas authentifié, et forcément : une ROM PXE n'a aucun
identifiant — la même nécessité qui gouverne déjà le point de réponse. Les contrôles sont
donc structurels : lecture seule, borné au catalogue, et aucun chemin de système de
fichiers n'est jamais construit à partir d'une requête. Plus un qui peut dire *pas vous* :

```console
$ export RESCRIPTUM_BOOT_ALLOW=10.0.0.0/8,192.168.0.0/16
```

Non définie, n'importe qui pouvant atteindre le port, ce qui sur un VLAN de
provisionnement est la configuration honnête. **Un VLAN de démarrage est la recommandation
qui fonctionne vraiment** ; voir [Sécurité](./security.md).

## Réglages

| Variable | Défaut | À quoi elle sert |
|---|---|---|
| `RESCRIPTUM_MEDIA_ADDR` | `0.0.0.0:8001` | Le listener |
| `RESCRIPTUM_MEDIA_TIMEOUT_SECS` | `600` | Échéance du transfert entier |
| `RESCRIPTUM_MEDIA_MAX_CONNECTIONS` | `16` | Transferts simultanés |

Seize, c'est bas volontairement. Chaque transfert retient son jeton pendant des minutes,
et le petit bout de ce sur quoi cela doit tourner est un NAS avec un disque mécanique :
seize transferts à 64 Kio par morceau font environ deux méga-octets de tampons, une
arithmétique qui doit tenir dans 512 Mo de RAM.

Sur une machine de datacenter, montez-la. Le point de réponse a son propre budget et
n'est touché dans aucun des deux cas.
