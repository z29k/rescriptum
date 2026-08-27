---
title: Synology DSM 7
description: La cible d'origine — un DS416j ARMv7 avec 512 Mo et pas de Docker. Une installation par Package Center, ce qu'elle fait et ne fait pas pour vous, et la route manuelle si vous la préférez.
sidebar:
  label: Synology DSM 7
  order: 2
---

# Synology DSM 7

Un Synology DS416j est la raison d'être de ce projet : ARMv7, 512 Mo de RAM, DSM 7, pas de
Docker. Un binaire statique sans runtime n'y est pas une préférence esthétique — c'est la
seule chose qui rentre.

DSM 7 fait tourner systemd, mais il n'offre aucun endroit supporté pour une unité à vous :
les fichiers de `/usr/lib/systemd/system` appartiennent à Synology, et une mise à jour de
DSM est libre de les remplacer. La route supportée vers un service, c'est un **paquet** —
installez-en un et DSM génère `pkgctl-rescriptum.service` à partir de lui. C'est par là que
cette page commence ; la [route par le planificateur de tâches](#sans-le-paquet) fonctionne
toujours et reste en bas.

## Installer le paquet

Téléchargez le `.spk` de votre modèle depuis la
[page des releases](https://github.com/z29k/rescriptum/releases) :

| Fichier | Pour |
|---|---|
| `rescriptum-<version>-armv7.spk` | DS416j et les autres modèles Marvell `armada38x` |
| `rescriptum-<version>-x86_64.spk` | tous les modèles Intel |

Un doute ? Demandez à la machine :

```console
$ ssh admin@nas synogetkeyvalue /etc.defaults/synoinfo.conf unique
synology_armada38x_ds416j
```

Puis **Package Center → Installation manuelle**, choisissez le fichier, et passez
l'avertissement disant que le paquet n'est pas vérifié par Synology. Cet avertissement ne
vise pas ce paquet en particulier : DSM 7 a supprimé la signature tierce et n'offre plus de
réglage de niveau de confiance, donc tout paquet non-Synology l'affiche. Notre vérification
à nous, c'est la somme SHA-256 publiée à côté du `.spk` :

```console
$ shasum -a 256 -c rescriptum-0.2.0-1-armv7.spk.sha256
```

L'assistant pose deux questions — **où vivent les réponses** et **sur quel port écouter** —
puis le paquet :

- crée un **dossier partagé `rescriptum`** et s'accorde un accès lecture/écriture dessus (si
  vous en avez déjà un de ce nom, il est conservé et gagne simplement le droit) ;
- crée le répertoire `answers` dedans à chaque démarrage ;
- **enregistre le port** auprès du pare-feu DSM, pour que le service soit sélectionnable par
  son nom ;
- lie **`rescriptum-cli`** dans `/usr/local/bin` ;
- démarre au boot, et s'arrête et redémarre depuis Package Center comme n'importe quoi
  d'autre.

## Ce que le paquet ne fait pas

Cinq choses à savoir avant qu'elles ne vous surprennent.

- **Il ne peut pas servir de TFTP.** Le port 69 est privilégié et DSM 7 n'autorise pas un
  paquet non signé à tourner en root : la livraison du chargeur revient donc au serveur
  TFTP de DSM — voir [Servir les médias d'installation, et le
  PXE](#servir-les-médias-dinstallation-et-le-pxe). Tout ce qui suit le chargeur
  appartient à ce paquet.
- **Il n'ouvre pas le pare-feu.** Enregistrer le port fait apparaître *rescriptum* par son
  nom dans l'éditeur de règles au lieu d'un numéro à taper. Si votre pare-feu est actif avec
  une règle par défaut qui refuse, il faut toujours créer la règle.
- **Il ne vous annonce pas les mises à jour.** Il n'y a pas de source de paquets à
  interroger — le modèle de distribution est : téléchargez le nouveau `.spk` depuis la page
  des releases et installez-le à la main, pour une mise à jour comme pour une première
  installation. Surveillez les releases.
- **Un chemin de réponses personnalisé, les permissions sont à vous.** Le paquet tourne sans
  privilèges et ne peut pas s'accorder l'accès à un dossier que vous nommez ; si vous
  pointez hors du partage `rescriptum`, donnez vous-même l'accès en lecture à l'utilisateur
  `rescriptum`.
- **Le droit sur le partage est réappliqué à chaque démarrage.** Si vous le restreignez
  délibérément, vous le retrouverez rétabli au démarrage suivant du paquet.

## Où vit quoi

| Quoi | Où | Survit à une mise à jour | Survit à la désinstallation |
|---|---|---|---|
| binaire, `rescriptum-cli`, le fichier d'environnement d'exemple | `/var/packages/rescriptum/target/` | non — remplacé | non |
| **le fichier d'environnement** | `/var/packages/rescriptum/etc/rescriptum.env` | **oui** | **oui** — voir ci-dessous |
| journal, pidfile, captures | `/var/packages/rescriptum/var/` | oui | **oui** |
| **les réponses** | `/var/packages/rescriptum/shares/rescriptum/answers/` | **oui** | **oui — toujours** |
| **la base SQLite**, si vous en utilisez une | à côté des réponses, dans le même partage | **oui** | **oui — toujours** |

Utilisez le chemin `shares/` plutôt que `/volume1/…` : c'est un lien symbolique maintenu par
DSM, donc il continue de marcher sur un NAS dont les données ne sont pas sur le volume 1.

La désinstallation laisse le dossier partagé et tout ce qu'il contient tranquilles. C'est à
la fois le comportement de DSM et le nôtre : quand le magasin est SQLite, la base *est* vos
réponses.

**Elle laisse aussi votre configuration derrière elle, et ça vaut d'être su.** `etc/` et
`var/` sont des liens vers `/volume1/@appconf/rescriptum` et `/volume1/@appdata/rescriptum`,
que DSM conserve — le fichier d'environnement reste donc sur le volume après la disparition
du paquet, **avec les jetons qu'il contient**. Une réinstallation le reprend, ce qui est
généralement ce qu'on veut. Si vous retirez rescriptum pour de bon et qu'il portait un
jeton, supprimez `/volume1/@appconf/rescriptum` vous-même.

## L'application de bureau

Le paquet installe une application sur le bureau DSM — l'icône est dans le menu principal,
et le bouton **Ouvrir** de Package Center y mène. C'est une vraie application DSM, bâtie sur
le framework d'interface du bureau : elle est dans le thème DSM et dans la langue de DSM.
Le français d'un DSM en français est aussi celui de l'application.

Elle a trois onglets :

- **Réglages** — chaque variable de configuration, sous forme de formulaire. Chaque champ
  dit d'où vient sa valeur, et une valeur définie dans l'*environnement* est affichée mais
  verrouillée, parce que modifier le fichier n'y changerait rien. Enregistrer écrit le
  fichier et propose de redémarrer le paquet, le serveur ne lisant sa configuration qu'une
  fois, au démarrage.
- **État** — la version, si le paquet tourne, le dossier des réponses et s'il est vraiment
  lisible *par l'utilisateur du service*, et la sortie de `check`.
- **Journal** — les dernières lignes du journal des requêtes et de `startup.log`.

Trois propriétés valent mieux d'être sues que découvertes :

- **Elle édite le fichier, pas le serveur qui tourne.** Elle fonctionne donc encore quand le
  serveur refuse de démarrer, c'est-à-dire précisément quand un panneau de réglages sert à
  quelque chose. Une modification qui laisserait le serveur incapable de démarrer est
  refusée avant toute écriture, avec la raison affichée.
- **Elle ne vous montre jamais un jeton.** `RESCRIPTUM_ANSWER_TOKEN` et
  `RESCRIPTUM_ADMIN_TOKEN` apparaissent comme *défini* ou *non défini*, et un champ vide
  veut dire « n'y touche pas », jamais « efface-le ». En saisir un nouveau le remplace.
- **Elle exige un administrateur DSM.** Être connecté à DSM ne suffit pas. Voir
  [sécurité](./security.md#lapplication-de-bureau) pour pourquoi ce contrôle est toute la
  porte.

*Redémarrer maintenant* arrête et redémarre le paquet via DSM lui-même, donc **DSM ferme la
fenêtre pendant ce temps** — rouvrez-la pour voir le nouvel état. L'application le dit à
côté du bouton plutôt que de vous laisser la surprise.

Elle demande **DSM 7.1 ou plus récent** (`os_min_ver="7.1-42661"`). Elle est bâtie sur le
framework ExtJS de DSM, présent en 7.1.1 comme en 7.2.2 — les deux mesurés. DSM 7.2 embarque
un framework Vue plus récent, et le guide actuel de Synology ne documente que celui-là ; mais
le DS416j qui justifie ce projet plafonne en 7.1.1, où `Vue` n'existe pas. ExtJS couvre donc
tous les DSM que ce paquet prend en charge plutôt que les seuls récents. Le 7.0 n'est pas
revendiqué : rien n'y a jamais tourné.

## Le configurer

L'application ci-dessus est la voie confortable. Tout ce qu'elle fait se fait aussi depuis
un shell, et sur une machine où le bureau n'est pas à portée c'est plus rapide :

```console
$ sudo rescriptum-cli config
env file: /var/packages/rescriptum/etc/rescriptum.env

  RESCRIPTUM_STORE            files                             default
  RESCRIPTUM_ANSWERS_DIR      /var/packages/rescriptum/shares/rescriptum/answers   file
  RESCRIPTUM_LISTEN_ADDR      0.0.0.0:8000                      file
  …

$ sudo rescriptum-cli config set RESCRIPTUM_LOG=problems
wrote /var/packages/rescriptum/etc/rescriptum.env
```

`config set` conserve les commentaires du fichier, décommente un réglage au lieu de le
dupliquer, et **refuse une modification qui empêcherait le serveur de démarrer**. Son code
de sortie dit si la configuration en est une sur laquelle le serveur démarrerait, ce qui le
rend utilisable depuis un script.

Dessous, c'est le même fichier, et l'éditer à la main reste parfaitement raisonnable :

```console
$ sudo vi /var/packages/rescriptum/etc/rescriptum.env
```

`postinst` l'écrit complet à une installation neuve, avec les variables utilisées
décommentées et les autres commentées avec une ligne disant à quoi elles servent.
**Arrêtez et redémarrez le paquet depuis Package Center pour appliquer un changement** — le
serveur lit le fichier à chaque démarrage.

Une mise à jour n'y touche jamais. L'exemple complet pour la version que vous avez est dans
`/var/packages/rescriptum/target/etc/rescriptum.env.example`, réécrit à chaque installation
et chaque mise à jour : c'est ainsi qu'une nouvelle variable devient visible sans déranger
votre fichier vivant. Toutes les variables sont dans la
[référence de configuration](../reference/configuration.md).

Le fichier est en `chmod 600` et appartient à l'utilisateur du paquet. C'est là que vivent
`RESCRIPTUM_ANSWER_TOKEN` et `RESCRIPTUM_ADMIN_TOKEN` et — étant sous `etc/` — c'est un
passager plausible d'une sauvegarde de configuration DSM. Mieux vaut le savoir que le
découvrir.

L'[API d'administration](./admin-api.md) est désactivée par défaut et, quand vous l'activez,
devrait rester sur la boucle locale et être atteinte par un tunnel SSH ; elle n'est
délibérément **pas** enregistrée auprès du pare-feu. Elle exige aussi
`RESCRIPTUM_STORE=sqlite` et un jeton d'au moins 16 caractères, deux **erreurs de
démarrage** — donc se tromper se manifeste par un paquet qui ne démarre pas, avec la raison
dans `/var/log/packages/rescriptum.log`.

## Mettre les réponses en place

Déposez les fichiers dans le répertoire `answers` du dossier partagé `rescriptum`, via File
Station ou en SSH, exactement comme ailleurs — voir
[écrire des réponses](../answers/index.md). Puis validez-les **en tant qu'utilisateur du
paquet** :

```console
$ sudo -u rescriptum rescriptum-cli check
```

Le `sudo -u` compte. Lancé en root, il réussit quoi que disent les permissions du dossier
partagé, ce qui rend un succès dénué de sens. `rescriptum-cli` est l'enveloppe fournie par
le paquet : elle nomme le fichier d'environnement, pour que `check` et `render` regardent
les réponses de cette machine plutôt que `/srv/answers`.

## Le pare-feu

**Panneau de configuration → Sécurité → Pare-feu** — créez une règle autorisant *rescriptum*
depuis votre réseau de provisionnement. Le service apparaît par son nom parce que le paquet
a enregistré son port.

Le pare-feu de DSM est la première raison pour laquelle une machine « ne contacte jamais le
serveur ».

Si vous changez le port plus tard, modifiez `RESCRIPTUM_LISTEN_ADDR` dans le fichier
d'environnement puis déplacez l'entrée du pare-feu, qui ne suit pas toute seule :

```console
$ sudo /usr/syno/sbin/synopkghelper update rescriptum port-config
```

## Servir les médias d'installation, et le PXE

Le paquet sait aussi servir l'installeur lui-même — noyaux, initrds et images — depuis le
NAS qui décide déjà la réponse. C'est éteint jusqu'à ce que vous l'allumiez :

1. Décommentez `RESCRIPTUM_MEDIA_DIR` dans le fichier d'environnement et redémarrez le
   paquet.
2. Posez une ISO dans le dossier `media` du partage `rescriptum`, via File Station ou SMB.
3. Enregistrez-la, pour qu'elle soit vérifiée et analysée une fois plutôt qu'à chaque
   requête :

```console
$ rescriptum-cli media add /volume1/rescriptum/media/proxmox-ve_8.4-1.iso \
    --sha256 9f86d081884c7d65…
$ rescriptum-cli media list
```

Le listener média est sur le **port 8001**, déjà déclaré au pare-feu à côté du port de
réponse — il reste à créer la règle.

**Aucune image n'est livrée avec le paquet**, et aucune ne le sera jamais : une ISO est
l'artefact de quelqu'un d'autre, elle pèse des gigaoctets, et elle évolue à son rythme. Ce
dossier est là où vous les gardez, et c'est **l'archive** — rien ici ne modifie une image
après son arrivée. Préparer une image Proxmox produit un fichier compagnon de deux cents
octets et une injection appliquée au fil de l'eau, donc les octets sur disque restent
exactement ce que Proxmox a publié et leur somme reste vérifiable contre celle de Proxmox.
Voir [Servir les médias de démarrage](./media.md).

### TFTP : celui de DSM, pas le nôtre

**Le paquet ne peut pas faire tourner de serveur TFTP, et ce n'est pas un oubli.** Le port
69 est privilégié, et DSM 7 n'autorise pas un paquet non signé à tourner en root — définir
`RESCRIPTUM_TFTP_ADDR` produirait donc un paquet qui refuse de démarrer. C'est documenté
comme indisponible dans le fichier d'environnement plutôt que proposé et cassé.

DSM a son propre serveur TFTP, et c'est le bon ici :

1. **Panneau de configuration → Services de fichiers → Avancé → TFTP** — activez-le, et
   définissez la racine sur le dossier `boot` du partage `rescriptum`.
2. Posez-y les chargeurs. Ils ne sont pas non plus dans le paquet — c'est iPXE, en GPLv2,
   et ils ont leur place à côté plutôt que soudés dedans. Depuis n'importe quelle machine
   Linux avec une chaîne de compilation C :

   ```console
   $ packaging/ipxe/build.sh --out /chemin/vers/rescriptum/boot
   ```
3. Faites pointer le DHCP vers ce NAS — **Panneau de configuration → Serveur DHCP → PXE**
   si le NAS sert le DHCP, ou votre propre serveur avec ce qu'imprime :

   ```console
   $ rescriptum-cli boot dhcp-snippet --format dnsmasq
   ```

**Tout ce qui suit le chargeur appartient à ce paquet.** Le chargeur enchaîne vers le port
8001, et à partir de là le menu, les réponses et les images sont tous servis par
rescriptum. DSM livre un fichier ; c'est toute sa part.

### Un réglage qui mérite d'être rempli

```
RESCRIPTUM_PUBLIC_HOST=192.168.1.10
```

Chaque script généré nomme cette adresse. Laissée vide, elle est déduite en interrogeant
la table de routage, et le panneau de réglages affiche ce que cela a donné plutôt qu'une
case vide — donc sur un NAS à une seule interface, il n'y a rien à remplir ici.

**C'est le NAS à deux interfaces qui mérite la lecture.** La déduction en retient une, et
le journal de démarrage nomme les autres à côté :

```
warning: RESCRIPTUM_PUBLIC_HOST is not set — derived 192.168.1.10, which is what every
generated URL will name. This host also has 10.0.0.10. If the machines reach it on one of
those instead, set it explicitly.
```

Se tromper produit une machine qui démarre, enchaîne, et se bloque sur une adresse qui
n'existe pas — long à diagnostiquer depuis la machine.

## Le journal

`RESCRIPTUM_LOG_FILE` pointe le serveur vers
`/var/packages/rescriptum/var/rescriptum.log`, et le paquet installe une strophe logrotate
pour lui — hebdomadaire, huit conservés, `copytruncate` (le serveur ouvre son journal une
fois et ne le rouvre jamais, donc tout le reste arrêterait silencieusement la
journalisation). À côté, `var/startup.log` contient ce que le serveur dit avant de savoir où
vit son journal : une erreur de configuration, un fichier d'environnement mal formé.

Une fois qu'un déploiement devient routinier, `RESCRIPTUM_LOG=problems` garde les échecs et
laisse tomber les réponses réussies, seule chose à fort volume là-dedans.

## Quand ça ne démarre pas

Trois endroits disent pourquoi, dans cet ordre :

```console
$ cat /var/log/packages/rescriptum.log        # la sortie des scripts du paquet
$ cat /var/packages/rescriptum/var/startup.log  # ce que le serveur a dit avant d'avoir un journal
$ cat /var/packages/rescriptum/var/rescriptum.log
$ systemctl status pkgctl-rescriptum          # ce qu'a vu le gestionnaire de services
```

Une **configuration refusée** — un jeton d'administration de moins de 16 caractères, un
magasin impossible à ouvrir — est signalée *après* que le serveur sait où vit son journal :
elle atterrit donc dans `rescriptum.log` ; un fichier d'environnement mal formé est signalé
avant, et atterrit dans `startup.log`. Le `start` du paquet affiche la fin des deux quand le
serveur sort immédiatement, pour que Package Center vous montre la raison et pas seulement
l'échec.

**DSM ne relance pas le processus s'il meurt.** L'unité qu'il génère est `Type=oneshot` avec
`RemainAfterExit=yes` et sans `Restart=` : un serveur qui sort reste arrêté jusqu'à ce que
vous le redémarriez depuis Package Center. Ce n'est pas une régression — la route par le
planificateur ne le relançait pas non plus — mais mieux vaut le savoir avant de compter
dessus.

Un paquet qui s'installe, démarre, puis répond `404` à tout, c'est presque toujours le
répertoire des réponses : vérifiez avec `sudo -u rescriptum rescriptum-cli check`. Sur un
NAS avec un dossier partagé chiffré, c'est aussi à cela que ressemble un démarrage avant que
le volume soit déverrouillé — déverrouillez-le et redémarrez le paquet.

## Vérifier

```console
$ curl http://IP_DU_NAS:8000/health
OK
```

## Sans le paquet

La route manuelle fonctionne toujours, et c'est le choix honnête si vous préférez ne rien
installer du tout.

Utilisez le build **`armv7-unknown-linux-gnueabihf`** (ou `x86_64-unknown-linux-musl`, ou
`aarch64-unknown-linux-musl` pour un modèle ARM plus récent) de la
[page des releases](https://github.com/z29k/rescriptum/releases), ou compilez-en un
vous-même (voir [construire](../../development/building.md)).

```console
$ scp rescriptum admin@nas:/volume1/netboot/rescriptum
$ ssh admin@nas chmod +x /volume1/netboot/rescriptum
$ ssh admin@nas mkdir -p /volume1/netboot/answers
```

Si ARMv7 se comporte mal, confirmez la vraie architecture avant de supposer :

```console
$ ssh admin@nas uname -m
armv7l
```

**Prenez le build ARMv7 publié, pas un build musl que vous auriez fait vous-même.** Le
binaire `armv7` publié est lié à la glibc 2.17, que DSM possède ; un build musl du même code
s'installe, répond à `--version`, puis meurt dès qu'il veut l'heure. Les noyaux 3.10 de
Synology répondent `EINVAL` aux appels *time64* là où musl 1.2 n'attend qu'`ENOSYS` pour se
replier — la [page de build](../../development/building.md#pourquoi-armv7-est-la-seule-cible-qui-ne-soit-pas-musl)
porte la mesure. Les builds x86_64 et aarch64 sont en musl statique et ne sont pas
concernés.

```console
$ file rescriptum
ELF 32-bit LSB pie executable, ARM, EABI5 version 1 (SYSV), dynamically linked, ...
```

`RESCRIPTUM_ANSWERS_DIR` vaut `/srv/answers` par défaut, qui n'existe pas sur DSM : il faut
donc la définir explicitement. Le fichier d'environnement ci-dessous est l'endroit le plus
propre pour le faire.

**Panneau de configuration → Planificateur de tâches → Créer → Tâche déclenchée → Script
défini par l'utilisateur**

| Champ | Valeur |
|---|---|
| Événement | **Démarrage** |
| Utilisateur | `root` |
| Commande | voir ci-dessous |

Si vous utilisez un jeton, **ne le mettez pas dans cette case.** Tout ce qui se trouve dans
les arguments d'un processus — et, dans le cas de DSM, dans la définition de la tâche — est
lisible par tous les utilisateurs de la machine via `ps`. Mettez la configuration dans un
fichier réservé à root et nommez-le :

```sh
# /volume1/netboot/rescriptum.env   (chmod 600, appartenant à root)
RESCRIPTUM_ANSWERS_DIR=/volume1/netboot/answers
RESCRIPTUM_LOG_FILE=/volume1/netboot/rescriptum.log
RESCRIPTUM_STORE=sqlite
RESCRIPTUM_DB_PATH=/volume1/netboot/answers.db
RESCRIPTUM_ADMIN_ADDR=127.0.0.1:8001
RESCRIPTUM_ADMIN_TOKEN=…
RESCRIPTUM_ANSWER_TOKEN=…
```

```sh
# l'entrée du planificateur de tâches exécute ceci
RESCRIPTUM_ENV_FILE=/volume1/netboot/rescriptum.env exec /volume1/netboot/rescriptum
```

**Préférez ceci au sourcing.** La forme plus ancienne —
`. /volume1/netboot/rescriptum.env && exec …` — fonctionne, et fonctionne toujours, mais
elle échoue *silencieusement* : oubliez le `.` du début, tapez une ligne de travers, ou
ratez les permissions, et le shell ne source rien pendant que le serveur démarre sur ses
**valeurs par défaut** — le répertoire de réponses par défaut, aucun jeton d'administration,
et pas un mot dans le journal. Avec `RESCRIPTUM_ENV_FILE`, le binaire lit le fichier
lui-même et **refuse de démarrer** s'il ne peut pas. Il prévient aussi si le fichier est
lisible par quelqu'un d'autre que root, et nomme toute clé qu'il ne reconnaît pas, de sorte
qu'un `RESCRIPTUM_ADMIN_TOKENN` est attrapé plutôt qu'ignoré en silence.

Les détails du format sont dans la
[référence de configuration](../reference/configuration.md#le-fichier-denvironnement).

Lancez la tâche une fois à la main depuis le planificateur plutôt que d'attendre un reboot
pour découvrir qu'elle ne marche pas. Puis ouvrez le port dans le pare-feu par son numéro,
et faites tourner le journal vous-même — le serveur ne le fait pas, et rien d'autre non
plus.

### Remplacer une instance en cours

```console
$ ./deploy.sh admin@nas
```

Il compile pour ARMv7, [vérifie les réponses d'abord](../answers/validating.md), copie le
binaire sous un nom temporaire pour qu'un fichier à moitié copié ne soit jamais exécuté, le
redémarre, et confirme que `/health` répond. Détails dans
[déploiement](./deployment.md#remplacer-une-instance-en-cours).

L'entrée du planificateur reste ce qui le démarre après un reboot — `deploy.sh` ne remplace
que ce qui tourne maintenant. Sur une installation par paquet, passez par Package Center.

## Arrêt

Les deux routes envoient `SIGTERM`, que le serveur gère : il arrête d'accepter et sort. Il
n'y a rien à perdre dans un cas comme dans l'autre.

## Ce qu'on peut attendre d'un DS416j

512 Mo et un cœur ARMv7, ce n'est pas grand-chose, et il n'y a pas besoin que ça le soit.
Mesuré sur un DS416j faisant tourner le paquet, à travers le réseau local : **3 à 4 ms pour
composer et servir une réponse**, aller-retour réseau compris, pour une machine revendiquée
par un groupe et fusionnée avec son propre fichier. Une connexion coûte des kilooctets
plutôt qu'un thread, le listing du répertoire est mis en
cache et invalidé par la mtime plutôt que parcouru à chaque requête, et un groupe sans
surcharge par machine est rendu une fois au chargement puis servi comme une chaîne
préparée.

La chose qui vaut d'être sue : le travail sur le système de fichiers se fait sur un pool de
threads bloquants, parce que `read_dir` sur un NAS dont le disque dort n'est pas un appel
rapide, et bloquer un worker asynchrone bloquerait toutes les autres connexions qu'il
pilote.
