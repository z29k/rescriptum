---
title: Allumer des machines
description: Dire à une machine de démarrer sur le réseau et de s'allumer, via Redfish ou par un script que vous fournissez — et la commande qui vérifie tout avant que quoi que ce soit ne bouge.
sidebar:
  label: Allumer des machines
  order: 10
---

# Allumer des machines

Tout le reste ici répond aux machines qui demandent. Cette partie-ci **émet** : elle dit à
un BMC d'armer un démarrage réseau et d'appuyer sur le bouton, pour qu'une installation
puisse être lancée depuis un terminal plutôt que depuis une chaise devant la baie.

C'est **éteint tant que `RESCRIPTUM_CONTROLLERS_FILE` ne nomme pas de fichier**. Non
défini, il n'y a aucun identifiant, aucune connexion sortante et aucun chemin de code qui y
mène. Un déploiement qui veut un pur serveur de réponses retrouve exactement ce qu'il
avait.

C'est aussi **synchrone et déclenché par un opérateur, toujours**. Rien ici ne réconcilie,
ne réessaie ni ne décide de son propre chef qu'une machine devrait être réinstallée. Chaque
action est une personne ou un script, une fois.

## Le fichier de contrôleurs

Un fichier TOML, indexé par le même identifiant que le répertoire des réponses — donc
`98:fa:9b:50:d8:10` et `98fa9b50d810` sont une seule machine des deux côtés.

```toml
# mode 0600. Pas dans le répertoire des réponses : c'est un .toml, et tout .toml servable à
# la racine de ce répertoire est un document de réponse.

["98-fa-9b-50-d8-10"]
kind   = "redfish"
url    = "https://10.0.0.51"      # schéma et hôte seulement — un chemin va dans `base`
base   = "/redfish/v1"            # PiKVM sert "/api/redfish/v1"
user   = "root"
pass   = "…"
pinnedpubkey = "sha256//…"        # ou cacert = "…", ou verify = false. Un des trois est exigé

["aa-bb-cc-dd-ee-ff"]
kind    = "command"               # tout ce que Redfish ne peut pas atteindre
on      = ["/usr/local/bin/pdu", "outlet", "7", "on"]
off     = ["/usr/local/bin/pdu", "outlet", "7", "off"]
pxe     = []                      # rien à faire — l'ordre de boot est réseau en permanence
timeout = 30                      # secondes ; un script bloqué bloquerait sinon `install`
```

Quatre règles méritent d'être connues avant d'en écrire un.

**Le serveur ne lit jamais ce fichier.** `power` et `install` le lisent. Un fichier
d'identifiants malformé ne peut pas arrêter l'écoute des réponses : les installations
d'une flotte tombant pour une raison sans rapport avec le fait de répondre, c'est
exactement l'échec qui rendrait cette fonctionnalité non rentable.

**Dites comment le certificat doit être vérifié.** Une entrée ne portant ni
`verify = false`, ni `cacert`, ni `pinnedpubkey` est refusée en nommant les trois. Les BMC
livrent des certificats auto-signés, donc « ne pas vérifier » est le défaut *commode* et
n'est donc pas celui que vous obtenez — la même règle que `media add`, où une URL exige
`--sha256` sauf si `--unverified` est passé. `pinnedpubkey` est la bonne réponse pour un
BMC auto-signé, et ne coûte rien.

**Un fichier lisible par le groupe est refusé à l'usage.** Pas averti : celui-ci porte des
identifiants capables de couper l'alimentation d'une baie. `chmod 600`. (Notez que les bits
de mode sous-estiment sur DSM, où une ACL peut accorder un accès dont `st_mode` ne parle
jamais — c'est donc un contrôle du mode, pas une preuve de confidentialité.)

**`on`, `off` et `pxe` sont des vecteurs d'arguments, jamais des lignes de commande.** Rien
ne passe par un shell, aucun découpage de mots n'a lieu, et rien de ce qu'une machine a
envoyé sur le réseau ne peut y arriver. Une chaîne y est refusée avec une explication
plutôt que découpée.

## Les commandes

```bash
rescriptum power list             # ce qui est configuré, joint au jeu de réponses
rescriptum power list --state     # ...et demander à chacun s'il est allumé
rescriptum power status <id>
rescriptum power on <id>
rescriptum power off <id>         # gracieux ; --hard force
rescriptum power pxe <id>         # armer un démarrage réseau unique, là où il y en a un
rescriptum install <id>           # vérifier, armer, pxe, allumer — le geste complet
rescriptum install <id> --dry-run # tout sauf l'allumage
```

`power list` **ne sonde pas**. Lire l'état, c'est un aller-retour HTTPS par contrôleur,
chacun jusqu'à son délai ; avec deux cents contrôleurs dont quelques-uns injoignables, une
liste qui demanderait prendrait des minutes et aurait l'air bloquée. `--state` est la
version qui demande, et elle est bornée et concurrente.

## `install`, et ce qu'il refuse

`install` est la commande pour laquelle le reste existe, et l'essentiel consiste à
vérifier. Allumer une machine qui démarre sur le réseau, enchaîne l'installateur et tombe
ensuite sur un 404 laisse un installateur planté à une invite dans une baie — l'échec que
tout ce projet existe pour empêcher.

Dans l'ordre :

1. **Chaque format que cette machine résout se rend**, gabarits remplis, aucun fait
   manquant. Pas une supposition sur celui vers lequel pointe le script de boot : tous.
2. **La politique est vérifiée**, et c'est là que ça s'arrête le plus souvent. Voir plus
   bas.
3. **Son script de boot est remis en place**, si une installation précédente l'a archivé
   dans un répertoire frère `installed-<id>/`. Votre document revient octet pour octet.
4. **Un démarrage réseau unique est armé**, là où le contrôleur en a un — et **relu pour
   confirmer qu'il a pris**.
5. **Elle est allumée, ou redémarrée**, selon l'état d'alimentation lu.

Trois refus, chacun pour une raison différente :

| Il dit | Parce que |
|---|---|
| *rien ne l'arme, elle resterait sur le menu de boot* | Avec `RESCRIPTUM_BOOT_UNCLAIMED=menu`, une machine non armée attend quelqu'un qui ne viendra pas, et brûle un cycle de démarrage |
| *…démarrerait son propre disque sans rien signaler* | Avec `local`, la même machine **ressemble exactement à une installation réussie**. C'est le cas dangereux |
| *son script de boot vient d'un groupe, et un groupe n'est jamais désarmé* | Voir plus bas |

### Pourquoi un groupe ne peut pas armer une installation

`POST /installed` déplace le `.ipxe` **propre à la machine** quand elle signale son succès
— jamais celui d'un groupe, délibérément, pour qu'une machine qui termine ne puisse pas
désarmer une baie entière.

La conséquence est facile à manquer. Une machine armée uniquement par son groupe
s'installe, signale son succès, n'est pas désarmée, et retrouve le même script de boot au
démarrage réseau suivant. Avec un ordre de boot réseau permanent, c'est une boucle de
réinstallation — et le webhook journalise `nothing was claiming it`, ce qui se lit comme si
tout avait fonctionné.

Donc `install` le refuse, `check` le signale par une note, et la correction consiste à
donner à la machine son propre document `.ipxe`. Gardez un `.ipxe` de groupe pour ce qui
est censé être servi indéfiniment : démarrer le disque local, ou un menu.

## Ce que chaque sorte de contrôleur sait faire

| Contrôleur | Alimentation | Démarrage réseau unique | Remarques |
|---|---|---|---|
| BMC serveur (iDRAC, iLO, Redfish générique) | oui | **oui** | |
| PiKVM | oui | **non** — il appuie sur des boutons, ce n'est pas le firmware | Son `PATCH` répond `204` sans rien changer ; la relecture l'attrape |
| JetKVM, PDU commutée, Wake-on-LAN | oui | non | Via `kind = "command"` |
| Intel AMT | oui | oui | Voir le piège plus bas |

**L'absence d'override de boot n'est pas une lacune.** Là où il n'y en a pas, laissez
l'ordre de boot sur le réseau en permanence et laissez le serveur décider si la machine
s'installe — ce que `RESCRIPTUM_BOOT_UNCLAIMED` et le désarmement `installed-` font déjà.
Un PiKVM plus rescriptum est une solution complète ; un BMC avec démarrage unique est une
ceinture en plus des bretelles.

## Ce qui mordra

**Un délai dépassé n'est pas un échec, c'est un inconnu.** Un reset qui a expiré a
peut-être allumé la baie. Rien ici ne réessaie une écriture automatiquement, et le message
dit que l'issue est inconnue plutôt que de laisser croire qu'il ne s'est rien passé. Relisez
l'état avec `power status`.

**Il n'y a pas de TLS dans ce binaire**, donc les appels Redfish passent par `curl`.
Contrairement à `media add`, il n'y a pas de repli sur `wget` : un appel Redfish demande un
POST avec un corps JSON, des en-têtes personnalisés et un identifiant tenu hors de la table
des processus, et wget ne fait rien de cette combinaison. L'identifiant est passé sur
l'entrée standard de curl, donc `ps` n'affiche que `curl --config -`.

**Un BMC devant plusieurs systèmes est refusé plutôt que deviné.** Un châssis lame, un Dell
FX2 et un PiKVM avec switch en exposent tous plusieurs ; prendre le premier allumerait la
machine de quelqu'un d'autre. Ajoutez `system = "…"` à l'entrée pour dire lequel.

**Intel AMT sur une carte réseau partagée peut affamer le DHCP de l'hôte.** Avec le
Management Engine qui tient l'interface sur une adresse statique pendant que l'hôte demande
un bail, le `dhclient` de l'installateur Proxmox abandonne au bout d'une dizaine de
secondes et l'installation échoue sur `Network is unreachable` — alors que
`dhclient -v eno1` depuis le shell de l'installateur réussit instantanément juste après.
Mettez AMT en DHCP. Rien ici ne peut élargir cette fenêtre.

## Ce que ce n'est pas

- **Pas un gestionnaire de configuration.** La frontière est le moment où SSH répond.
  C'est le terrain d'Ansible.
- **Pas une boucle de réconciliation.** Aucun agent ne décide qu'une machine devrait être
  réinstallée.
- **Pas un fan-out.** Il n'y a pas de forme « groupe », et si elle arrive un jour elle sera
  séquentielle avec un délai réglable — allumer quarante machines d'un coup est un
  événement électrique avant d'être un événement logiciel, et les datacenters échelonnent
  les mises sous tension à cause du courant d'appel.
- **Jamais une requête.** Aucun point d'entrée HTTP n'allume quoi que ce soit. L'endpoint
  de réponses est non authentifié par nécessité, et câbler le contrôle d'alimentation à
  proximité, c'est ainsi qu'un serveur de provisioning devient une arme.
