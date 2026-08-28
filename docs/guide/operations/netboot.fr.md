---
title: Démarrer une machine par le réseau
description: TFTP, le chargeur, le menu — toute la chaîne de la mise sous tension à l'installation sans surveillance, avec deux options ajoutées à un serveur DHCP que vous exploitez déjà.
sidebar:
  label: Démarrage réseau
  order: 9
---

# Démarrer une machine par le réseau

Une machine s'allume. Quatre maillons plus tard, elle s'installe comme quelqu'un l'a
décidé — ou, si personne n'a encore rien décidé à son sujet, elle attend dans un menu où
un humain peut le faire.

```
 mise sous tension
    │
(1) ├── le DHCP dit d'où démarrer ............ À EUX. Deux options, et nous
    │   générons l'extrait qui les pose.
    ▼
(2) ├── TFTP livre un chargeur ............... À NOUS
    │   un iPXE adapté à l'architecture, qui enchaîne via ${next-server}
    ▼
(3) ├── iPXE demande quoi faire .............. À NOUS
    │   machine connue   → sa propre réponse sans surveillance
    │   machine inconnue → le menu
    ▼
(4) └── les octets arrivent .................. À NOUS
        noyau, initrd, l'image elle-même — HTTP avec plages
```

**Le maillon 1 appartient à quelqu'un d'autre et cela ne changera pas.** rescriptum ne
parle pas DHCP du tout — ni serveur, ni proxy, ni derrière un drapeau. Les sites qui
déploient ceci en ont déjà un, et le faire pointer vers un serveur de démarrage est un
problème résolu depuis trente ans.

## Mise en route

```console
$ export RESCRIPTUM_MEDIA_DIR=/srv/media     # les images
$ export RESCRIPTUM_BOOT_DIR=/srv/boot       # les chargeurs
$ export RESCRIPTUM_PUBLIC_HOST=192.0.2.10   # ce que nommeront les scripts générés
```

`RESCRIPTUM_BOOT_DIR` dit où sont les chargeurs : non définie, il n'y a aucun listener
TFTP et rien sur `/boot/…`. La nommer démarre TFTP sur `0.0.0.0:69` sauf si vous dites le
contraire.

Le port 69 est privilégié, et c'est le *seul* port privilégié que ce serveur demandera
jamais — sans répondeur DHCP, il n'y a rien après 67 ni 4011. Quatre façons de traiter la
question, toutes portables :

```console
$ export RESCRIPTUM_USER=rescriptum          # démarrer en root, lier, puis abandonner
$ setcap cap_net_bind_service=+ep rescriptum # ou n'accorder que cette capacité
$ export RESCRIPTUM_TFTP_ADDR=0.0.0.0:6969   # ou le déplacer, si leur DHCP sait le dire
$ export RESCRIPTUM_TFTP_ADDR=off            # ou n'avoir aucun listener du tout
```

**`off` est une valeur, pas une absence** — c'est ainsi qu'on dit qu'un autre service de
cette machine livre le chargeur pendant que rescriptum sert le reste de la chaîne. Les
chargeurs restent servis en HTTP sur `/boot/…` et restent vérifiés par `boot check` ; seul
le listener disparaît. C'est un contournement de déploiement pour qui le veut, **jamais la
façon dont quoi que ce soit est livré ici** : c'est rescriptum le serveur TFTP, et une
version qui le couperait par défaut aurait cédé la chose même qu'elle est. Le [paquet
Synology](./synology.md) ouvre le port 69 avec un `setcap`.

**Un port TFTP qu'on ne peut pas lier n'arrête pas le serveur**, et c'est le seul endroit
où la règle « un listener qui ne peut pas se lier est fatal » s'inverse dans ce projet. Le
port 69 est le seul port privilégié de la conception, donc le seul bind qui puisse échouer
pour quelque chose que personne n'a configuré — une capacité qu'une mise à jour a
discrètement perdue, le plus souvent. Les réponses sont le produit ; mourir ici ferait
échouer toutes les installations en cours pour signaler qu'un second port n'a pas pu être
ouvert. Donc il avertit, continue de servir, et `boot check` sort en non-zéro :

```console
$ rescriptum boot check
  BROKEN nothing answers on 0.0.0.0:69 and it cannot be bound either: Permission denied.
  Port 69 is privileged: run as root and set RESCRIPTUM_USER to drop afterwards, or grant
  the binary cap_net_bind_service with setcap — the server still answers and still serves
  media, but a machine sent here by DHCP asks for a loader and gets nothing
```

Il demande un vrai chargeur au port plutôt que d'essayer de le lier, car lier prouve le
contraire de ce qu'on croit : un bind qui *réussit* signifie que personne n'écoute, et un
bind qui échoue ne distingue pas ce serveur d'un autre service qui squatterait le port.

**On lie d'abord, on abandonne ensuite**, toujours. L'ordre inverse fonctionne en test
sous root et échoue au déploiement, à un redémarrage — le seul moment où personne ne
regarde.

## Les deux lignes de leur serveur DHCP

```console
$ rescriptum boot dhcp-snippet --format dnsmasq
# rescriptum 0.2.0 - boot handoff for 192.0.2.10
# Architecture values are IANA option 93 codes; see docs/guide/boot/dhcp.
# Generated from the same table the TFTP server serves from.
dhcp-match=set:bios,option:client-arch,0
dhcp-match=set:efi64,option:client-arch,7
dhcp-match=set:efi64,option:client-arch,9
dhcp-match=set:efiarm64,option:client-arch,11
…
```

`--format` couvre `dnsmasq`, `isc`, `kea`, `powershell`, `pfsense` et `mikrotik` ;
`--one-loader` produit la forme d'une seule ligne pour un parc d'une seule architecture.

**L'extrait et le serveur TFTP sont générés depuis une même table**, de sorte que ce que
vous collez et ce que le serveur distribue ne peuvent pas diverger. Ce qu'ils *peuvent*
faire, c'est nommer un chargeur que personne n'a encore téléchargé, et cela échoue
silencieusement au niveau de la ROM : la machine demande, ne reçoit rien, et s'arrête
sans un message sur aucune console. Une commande l'attrape :

```console
$ rescriptum boot check
checking boot assets in /srv/boot
  ok   ipxe-arm64.efi (1.0M)
  MISSING ipxe-undionly.kpxe — every machine the snippet sends here will ask for it,
  get nothing, and stop
```

Son code de sortie est un contrat, comme celui de `check`. Placez-le au même endroit.

### Quatre détails que l'extrait généré traite correctement

Chacun est une façon d'échouer sans bruit sur le réseau de quelqu'un d'autre, et aucun
n'est évident :

- **Le champ BOOTP `file` *et* l'option 67.** Certaines ROM ne lisent que l'un des deux,
  et lequel n'est pas prévisible d'après le fournisseur.
- **Un défaut sans étiquette à la fin.** Chaque ligne d'architecture est étiquetée : une
  ROM qui n'envoie pas d'option 93 ne correspondrait à rien et n'obtiendrait aucun
  fichier de démarrage.
- **`HTTPClient` renvoyé dans l'option 60** pour les clients UEFI HTTP Boot. Le firmware
  *filtre les offres* dessus : une réponse ne portant que l'URL est écartée, en silence,
  ce qui est indiscernable d'une absence de serveur DHCP.
- **Un next-server pour ces clients aussi**, bien qu'ils récupèrent en HTTP. Sans lui, le
  script embarqué du chargeur lit un `${next-server}` vide et enchaîne vers nulle part.

::: tip Windows Server
Une stratégie DHCP **ne peut pas se conditionner sur l'option 93** — les types de
condition sont la classe fournisseur, la classe utilisateur, la MAC, l'identifiant
client, le FQDN et les informations de relais. L'architecture n'atteint une stratégie
qu'à l'intérieur de la chaîne de l'option 60, donc le PowerShell généré définit des
classes fournisseur sur `PXEClient:Arch:00007*` et y accroche les stratégies. Même
résultat, mécanisme différent, et c'est exactement le genre de chose qu'on retient à
moitié.
:::

## Le chargeur

TFTP livre **un fichier**, et la règle est écrite dans le code :

> **TFTP livre le chargeur. Tout ce qui suit passe en HTTP.**

À 1468 octets par aller-retour, TFTP déplace environ 1,4 Mo/s sur une milliseconde de
latence. Le chargeur fait un mégaoctet : deux secondes. Une image de 1,5 Go prendrait
près de vingt minutes, contre quinze secondes en HTTP sur le même câble.

Quel chargeur dépend de ce que le firmware a annoncé :

| Option 93 | Client | Servi |
|---|---|---|
| `0x0000` | BIOS PXE | `ipxe-undionly.kpxe` |
| `0x0007`, `0x0009` | UEFI x86-64 | `ipxe-x86_64.efi`, plus `-snp` / `-snponly` |
| `0x000b` | UEFI ARM64 | `ipxe-arm64.efi` |
| `0x0010`, `0x0013` | UEFI HTTP Boot | les mêmes fichiers, en HTTP, sans TFTP du tout |
| tout le reste | UEFI 32 bits, EBC, U-Boot | refusé, avec la raison |

`0x0009` mérite un mot. La RFC 4578 le définissait comme « EFI x86-64 » ; le registre
IANA, réécrit par la RFC 5970, le liste comme « EBC ». Les vrais firmwares x64 envoient
l'un ou l'autre, donc les deux pointent vers x64 — une table produite depuis le seul
registre ne donnerait rien à la moitié d'un parc.

`snponly` existe parce que la construction UEFI ordinaire ne voit pas toujours la carte
réseau. Toutes les variantes sont servies et la table choisit ; c'est précisément le
savoir qu'un exploitant ne devrait pas avoir à acquérir.

### Se les procurer

Chaque version publiée attache `rescriptum-boot-assets-<version>.tar.gz`. Décompressez-le
là où le serveur peut le lire, nommez le répertoire, et vérifiez-le :

```console
$ tar -xzf rescriptum-boot-assets-0.2.0.tar.gz -C /srv
$ export RESCRIPTUM_BOOT_DIR=/srv/rescriptum-boot-assets-0.2.0
$ rescriptum boot check
```

Il contient les huit chargeurs, un `SHA256SUMS`, un `ipxe.iso` et un `ipxe.usb`
démarrables pour une machine sans ROM PXE utilisable, et un `NOTICE` — c'est iPXE, en
GPLv2, construit depuis un commit amont épinglé. **C'est un téléchargement séparé, et il
ne fait partie d'aucune archive binaire ni d'aucun `.spk`**, délibérément : des fichiers
séparés servis à côté relèvent de la simple agrégation, et `packaging/ipxe/` est l'offre
écrite qui les accompagne.

Pour les construire vous-même à la place — le même script que la release exécute, depuis
le même épinglage :

```console
$ packaging/ipxe/build.sh --out /srv/boot
```

Un chargeur venu d'ailleurs convient aussi, à condition qu'il enchaîne vers *ce* serveur
plutôt que vers Internet — voir ci-dessous pourquoi un chargeur d'origine ne le fait pas.

## Ce qui se passe au *deuxième* démarrage

La première question que tout le monde se pose après une installation réussie, et elle a
une vraie réponse.

Une machine qui vient d'être installée redémarre, et si le démarrage réseau est encore
premier dans son BIOS, elle revient ici. Ce qui suit est décidé par un seul réglage :

| `RESCRIPTUM_BOOT_UNCLAIMED` | Une machine qu'aucune réponse ne revendique |
|---|---|
| `menu` (défaut) | reçoit le menu, dont la première entrée est le disque local et dont le délai y retombe — quinze secondes, puis le disque |
| `local` | est rendue directement à son firmware, qui passe au périphérique suivant |

**Ce sont deux lectures opposées de ce que signifie un fichier de réponse**, et le choix
appartient au déploiement.

Avec le menu, un fichier qui revendique une machine est la façon de dire *laisse celle-ci
tranquille* — car sans lui elle atterrit dans un menu que quelqu'un pourrait cliquer. C'est
juste pendant qu'on provisionne, et c'est la thèse du projet : une machine dont personne
n'a rien décidé doit finir là où un humain peut décider.

Avec `local`, un fichier de réponse veut dire *installe celle-ci*, et son absence est
l'état sûr. Il n'arrive rien à une machine pour laquelle vous n'avez pas écrit de fichier —
elle démarre sur son disque, à chaque fois, sans menu à cliquer par accident. C'est la
lecture dont un parc en production a besoin, et c'est celle qui passe à l'échelle : le
nombre de machines qu'on veut réinstaller est toujours plus petit que celui des autres.

**Le bénéfice, c'est que le démarrage réseau peut rester premier dans le BIOS pour
toujours.** Réinstaller une machine devient *ajouter un fichier, redémarrer* — sans
console, sans menu de démarrage, sans toucher au matériel. Retirer le fichier est ce qui
empêche que cela se reproduise.

```console
$ rescriptum config set RESCRIPTUM_BOOT_UNCLAIMED=local
```

Dans les deux cas l'identité de la machine part d'abord. Le réglage décide seulement de ce
qui arrive quand rien ne l'a revendiquée — pas s'il faut demander.

## Installer une machine une fois, et une seule

Une machine revendiquée par une réponse `.ipxe` s'installe, redémarre, est revendiquée de
nouveau, et se réinstalle — en effaçant son disque à chaque tour. Tous les systèmes de
provisionnement répondent pareil : une machine est *armée* pour l'installation, et quelque
chose la désarme ensuite.

**C'est la machine qui sait.** Proxmox appelle un webhook après une installation réussie et
**avant le redémarrage**, avec ses interfaces réseau dans le corps :

```toml
[post-installation-webhook]
url = "http://192.0.2.10:8000/installed"
auth-token = "nas:s3cr3t"
```

```console
$ rescriptum config set RESCRIPTUM_INSTALLED_TOKEN=nas:s3cr3t
```

C'est tout. La machine termine, elle le dit, et `98fa9b50d810.ipxe` devient
`installed-98fa9b50d810.ipxe` — qui ne lui correspond plus, le préfixe faisant partie du
nom comparé. Elle démarre sur son disque désormais, et la réarmer consiste à renommer le
fichier dans l'autre sens.

**Pas de jeton, pas d'endpoint** — absent plutôt qu'ouvert. Sans lui, `/installed` est une
demande de réponse ordinaire comme n'importe quel chemin, ce qui permet à une URL de rester
gravable dans une ISO.

Trois choses qu'il ne fait pas, et chacune est délibérée :

- **Il ne touche jamais un groupe.** Un groupe revendique un rack entier, et une machine
  qui finit son installation ne doit pas désarmer ses voisines. La recherche ne consulte
  pas les groupes du tout, plutôt que de les écarter après coup.
- **Il ne touche rien d'autre que le `.ipxe`.** Le `.toml` de la machine est ce que
  l'installateur a lu pour la construire, et il reste comme trace de la manière.
- **Il déplace, il ne supprime pas.** C'est le seul chemin où quelque chose venu du réseau
  modifie le jeu de réponses : rien de ce qu'il fait n'est irréversible.

Arriver deux fois n'est pas une erreur — un webhook peut être réessayé, et une machine
installée depuis le menu n'a jamais été revendiquée. Un désarmement qui *échoue* est
journalisé en `still armed`, parce que sa conséquence est autrement silencieuse : la
machine se réinstalle au démarrage suivant et rien d'autre ne le dirait.

### Toutes les autres familles rapportent aussi

Proxmox est le seul à avoir son propre webhook. **La revendication n'est pas propre à
Proxmox** — c'est un document `.ipxe`, qui concerne le chargeur et non le système
d'exploitation — donc toutes les familles ont besoin du même désarmement, et toutes ont un
endroit où lancer une ligne à la fin de leur installation :

```bash
curl -fsS -X POST -H "Authorization: Bearer nas:s3cr3t" \
  "http://192.0.2.10:8000/installed?mac=$(cat /sys/class/net/*/address | head -1)"
```

Pas de corps, pas de JSON : la query dit quelle machine, l'en-tête dit qu'elle en a le
droit. Où mettre cette ligne :

| Famille | Où |
|---|---|
| Proxmox | `[post-installation-webhook]` — natif, rien à écrire |
| Debian | `d-i preseed/late_command string in-target sh -c '…'` |
| Ubuntu | `late-commands:` dans le document autoinstall |
| RHEL, AlmaLinux, Rocky | la section `%post` du kickstart |
| SUSE | `<scripts><chroot-scripts>` du profil AutoYaST |

**Prenez la MAC de l'interface qui a démarré, pas la première par ordre alphabétique.**
L'exemple ci-dessus prend la première entrée de `/sys/class/net`, ce qui va sur une machine
à une carte et se trompe sur une machine à quatre — et une mauvaise MAC désarme la mauvaise
machine, ou personne. Sur une machine à plusieurs cartes, nommez l'interface.

L'endpoint accepte les deux formes du justificatif : l'`auth-token` de Proxmox arrive dans
le corps JSON parce que c'est ce que Proxmox envoie, et un en-tête bearer parce que c'est
ce qu'envoie un script shell. Même secret, même comparaison en temps constant.

## Quand tout est juste et que la machine refuse quand même de s'installer

La chaîne peut être parfaite et échouer à la dernière marche, côté machine et non côté
serveur. Deux cas réellement rencontrés, tous deux sur un Lenovo vPro :

**Intel AMT en adresse statique, sur une carte partagée avec le système.** Le `dhclient` de
l'installateur envoie deux requêtes à onze secondes d'intervalle puis abandonne ; si le
Management Engine tient l'interface avec une configuration statique pendant que l'hôte
demande du DHCP, ces onze secondes passent sans offre et l'installation s'arrête sur
`Fetching answer file via HTTP failed: Network is unreachable`. **Mettez l'AMT en DHCP
aussi.** Lancer `dhclient -v eno1` à la main depuis le shell de l'installateur réussit
ensuite immédiatement, et c'est ce qui rend le diagnostic déroutant : le réseau va bien,
c'est la temporisation qui ne va pas.

**Un port de commutateur qui ne transmet pas tout de suite**, pour la même raison et avec
le même symptôme — RSTP en convergence, ou un lien encore en négociation après que le noyau
a repris la carte des mains d'iPXE. Ce serveur n'y peut rien dans les deux cas : les onze
secondes sont la fenêtre de l'installateur, pas la nôtre.

L'installateur ouvre un shell root quand il abandonne, et ce shell est le diagnostic le
plus rapide qui soit :

```console
# ip link                 # l'interface est-elle seulement montée ?
# dhclient -v eno1        # une offre revient-elle quand on la demande à la main ?
# ip addr show eno1
```

Une adresse qui apparaît là et pas pendant l'installation veut dire que le réseau
fonctionne et que la machine a simplement demandé trop tôt.

## Comment iPXE finit par parler à *nous*

La question qu'on ne s'attend pas à devoir trancher. Quel que soit le livreur du
chargeur :

- Un `undionly.kpxe` ordinaire venu d'ipxe.org fait du DHCP, se fait dire de charger
  iPXE, et **se recharge lui-même indéfiniment** — la boucle d'enchaînement documentée
  par iPXE.
- Un binaire netboot.xyz d'origine embarque un script qui va droit au menu **public**
  `boot.netboot.xyz`. Pas de boucle, mais votre menu et vos réponses ne sont jamais
  consultés.

Les chargeurs que rescriptum distribue portent un script de trois lignes qui enchaîne via
`${next-server}` — la valeur que l'option 66 a déjà posée, puisque c'est ainsi que le
chargeur est arrivé. Une seule construction générique fonctionne donc dans tous les
déploiements, sans seconde condition dans un fichier de configuration qui appartient à
quelqu'un d'autre.

Le script enchaîne vers le **port 8001**, et c'est un contrat plutôt qu'une préférence :
il est gravé dans le chargeur avant qu'aucun déploiement n'existe et ne peut lire aucune
configuration. Déplacer `RESCRIPTUM_MEDIA_ADDR` est permis, et `boot check` le signale.

## Ce qu'une machine voit

L'étape deux met l'identité de la machine dans la chaîne de requête, la seule chose que
DHCP ne peut pas faire — une option DHCP ne peut pas porter `${net0/mac}` :

```console
$ rescriptum boot bootstrap
#!ipxe
chain http://192.0.2.10:8000/ipxe/boot?mac=${netX/mac}&uuid=${uuid}\
&serial=${serial:uristring}&asset=${asset:uristring}\
…
|| chain http://192.0.2.10:8001/ipxe/menu
```

Deux détails y sont porteurs. **`netX`, pas `net0`** — `net0` n'est que la première
interface, donc un serveur démarrant par son second port s'identifierait par le premier,
inutilisé. Et **`:uristring`** sur chaque chaîne SMBIOS, parce que `${manufacturer}`
s'étend en `Dell Inc.` avec l'espace et qu'iPXE n'encode rien de lui-même.

Ce `||` final, c'est tout « un menu est la réponse par défaut » : une machine que quelque
chose réclame reçoit sa propre réponse sans surveillance, et une machine que rien ne
réclame retombe sur le menu. C'est la description de poste de `default.toml`, mot pour
mot, appliquée à un autre format.

## Le menu

```console
$ rescriptum boot menu
```

Rendu depuis le catalogue **au moment de la requête**, et non maintenu comme un fichier :
posez une ISO dans le répertoire de médias et elle est dans le menu à la requête
suivante.

- **« Boot from the local disk » est en premier, et le délai y retombe.** Une machine qui
  démarre en PXE par accident, et que rien ne réclame, finit sur son propre disque au
  bout de quinze secondes. Elle n'attend jamais un humain qui ne vient pas, et elle
  n'installe jamais rien. Avec la règle qui veut qu'une machine non réclamée reçoive un
  menu plutôt qu'une installation, **le pire cas d'une erreur sur le périmètre des
  machines qui atteignent ce serveur est quelques secondes ajoutées à un démarrage.**
- Les entrées sont **filtrées sur l'architecture du client** : une image ARM64 n'est pas
  proposée à une machine x86 — ce serait une entrée qui démarre le mauvais noyau.
- Une image qu'aucune détection n'a su placer est quand même proposée, comme un CD.
- Les entrées de diagnostic — un shell, `netinfo`, et une qui démarre un *autre*
  rescriptum — sont ce dont tout serveur de démarrage finit par avoir besoin. La dernière
  sert à tester un serveur candidat sur site, depuis celui qui tourne, sans toucher au
  DHCP ni aux chargeurs.

`RESCRIPTUM_BOOT_TIMEOUT_SECS` (15 par défaut) règle l'attente, et
`RESCRIPTUM_BOOT_TITLE` la barre de titre. Le logo est récupéré par
`console --picture … ||`, qui **tolère son propre échec** : une console série via IPMI
n'a pas de framebuffer, et c'est ainsi que la moitié des installations en datacenter sont
suivies.

## Ce qui casse quand ce serveur est arrêté

Cela mérite d'être dit franchement, parce que « serveur de démarrage » sonne critique et
ne l'est pas :

| | rescriptum arrêté |
|---|---|
| Adressage DHCP, DNS, routage | **inchangés** — il ne parle aucun de ces protocoles |
| Machines déjà installées et en service | **inchangées** |
| Machines qui redémarrent | **inchangées** — elles démarrent sur disque |
| Une machine qui démarre en PXE par accident | passe au périphérique suivant, comme elle l'aurait fait |
| Démarrer une *nouvelle* installation | s'arrête |

**Rien de ce que rescriptum installe ne dépend de rescriptum ensuite.** Le point de
réponse est consulté pendant une installation et plus jamais.

## Sécurité

Le trafic de démarrage n'est pas authentifié, et forcément — une ROM PXE n'a aucun
identifiant, la même nécessité qui gouverne déjà le point de réponse. Les contrôles sont
donc structurels, et l'un d'eux peut dire *pas vous* :

```console
$ export RESCRIPTUM_BOOT_ALLOW=10.0.0.0/8    # partagée par TFTP et les médias
```

UDP est falsifiable et TFTP est de l'UDP, donc le serveur **ne répond jamais à une
destination de diffusion ou de multidiffusion** — de l'hygiène anti-amplification plutôt
que de la politesse —, plafonne les transferts au total et par pair, et journalise chacun
d'eux. Il est en lecture seule : une requête d'écriture est refusée comme violation
d'accès, car écrire un chargeur en UDP non authentifié serait un moyen de changer ce que
démarre chaque machine du segment.

**Un VLAN de démarrage est la recommandation honnête** et celle qui fonctionne vraiment.
Voir [Sécurité](./security.md).

::: tip Secure Boot
Nos chargeurs ne sont pas signés, et shim ne charge que ce que la clé de son
distributeur a signé — servir un shim à côté d'un iPXE non signé n'est donc pas un
support de Secure Boot, c'est un démarrage qui s'arrête sur une erreur de signature. Ce
qui fonctionne : désactiver Secure Boot, enrôler une MOK, ou laisser le firmware démarrer
en PXE le shim et le GRUB signés *de la distribution cible*, servis par le listener média
comme n'importe quel fichier. Nous ne signons rien, ne retirons rien, et rien ici
n'affaiblit une machine dont Secure Boot est actif.
:::

## Quand leur DHCP est vraiment intouchable

Rien de tout cela ne coûte une ligne de code, et les trois fonctionnent :

- **UEFI HTTP Boot avec une URL saisie dans le firmware.** Les firmwares serveur récents
  permettent d'entrer directement une URL de démarrage. La chaîne commence alors sur le
  listener média, sans aucune option DHCP.
- **iPXE depuis un média virtuel IPMI, une clé USB ou la ROM de la carte réseau**,
  portant l'adresse de ce serveur. Une image d'un mégaoctet, montée une fois par machine.
- **dnsmasq en mode proxy-DHCP**, pour un site qui a vraiment un serveur DHCP qu'il ne
  peut pas modifier. Il existe, il est mature, il tient en trois lignes de configuration,
  et ce n'est pas à nous de le réécrire. Le nommer est la réponse honnête.
