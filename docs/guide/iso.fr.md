---
title: Préparer les médias d'installation
description: L'URL à graver dans chaque ISO — et pourquoi son chemin décide quels documents peuvent répondre.
sidebar:
  label: Médias d'installation
  order: 3
---

# Préparer les médias d'installation

Chaque installateur se voit dire, au moment de la fabrication du média, où récupérer sa
configuration. Cette URL fait ici deux choses :

1. **Elle atteint le serveur.** N'importe quel chemin fonctionne — `POST` et `GET` sont
   traités sur tous, précisément pour que l'URL gravée dans une ISO ne soit jamais fausse.
2. **Son chemin déclare le format.** Un segment nommant un alias connu restreint la réponse
   aux documents de ce format, pour qu'un client kickstart ne reçoive pas du TOML.

Donnez à chaque installateur son URL, et un seul serveur répond pour tous.

## Proxmox VE

```console
$ proxmox-auto-install-assistant prepare-iso proxmox-ve.iso \
    --fetch-from http \
    --url http://SERVER:8000/proxmox/answer \
    --output proxmox-auto.iso
```

L'installateur POST un inventaire JSON du matériel trouvé et attend la réponse dans le corps
de la réponse HTTP. `/proxmox/` restreint la réponse aux documents `.toml` ; `/answer` seul
ne nomme aucun alias et ne contraint rien, ce qui est pourquoi un déploiement existant
continue de fonctionner sans changement.

Pour exiger une authentification, préparez l'ISO avec un jeton et donnez le même au serveur :

```console
$ proxmox-auto-install-assistant prepare-iso proxmox-ve.iso \
    --fetch-from http --url http://SERVER:8000/proxmox/answer \
    --answer-auth-token 'une-longue-chaine-aleatoire' --output proxmox-auto.iso

$ export RESCRIPTUM_ANSWER_TOKEN='une-longue-chaine-aleatoire'
```

Voir [Sécurité](./operations/security.md) pour ce que cela protège et ce que cela ne protège
pas.

### Sans refabriquer l'ISO

Proxmox peut aussi découvrir l'URL au démarrage, ce qui évite de refabriquer le média quand
l'adresse change :

- un enregistrement DNS TXT sur `proxmox-auto-installer.<votre-domaine>`, ou
- l'option DHCP 250.

Les deux sont hors du périmètre de ce serveur — il lui suffit d'être à l'adresse qu'ils
nomment.

## Tout le reste, via iPXE

Les autres installateurs *récupèrent* leur configuration et s'identifient dans la query
string, parce qu'iPXE substitue ses propres variables dans l'URL avant de la chercher :

| Variable | Vaut |
|---|---|
| `${net0/mac}` | l'adresse MAC de la première carte réseau |
| `${uuid}` | l'UUID système SMBIOS |
| `${serial}` | le numéro de série système |
| `${manufacturer}`, `${product}` | fabricant et modèle DMI |

Ces valeurs deviennent des [faits](./answers/selection.md#les-faits-quun-sélecteur-peut-tester)
sur lesquels un document peut être sélectionné, et alimentent aussi la botte de foin —
donc un document nommé d'après une MAC résout que la MAC soit arrivée dans un corps POST ou
dans une query string.

| Installateur | Paramètre de boot |
|---|---|
| **RHEL / CentOS / Fedora / Alma / Rocky** | `inst.ks=http://SERVER:8000/rhel/ks?mac=${net0/mac}` |
| **Debian preseed** | `url=http://SERVER:8000/debian/preseed?mac=${net0/mac}` |
| **Ubuntu autoinstall** | `autoinstall ds=nocloud-net;s=http://SERVER:8000/ubuntu/?mac=${net0/mac}` |
| **Flatcar / Fedora CoreOS** | `ignition.config.url=http://SERVER:8000/flatcar/config?mac=${net0/mac}` |
| **openSUSE / SLES** | `autoyast=http://SERVER:8000/suse/profile?mac=${net0/mac}` |
| **Windows** | récupéré par votre propre outillage depuis `http://SERVER:8000/windows/unattend` |

Un fragment de script iPXE complet :

```
#!ipxe
set base http://SERVER:8000
kernel ${base}/images/rhel9/vmlinuz inst.ks=${base}/rhel/ks?mac=${net0/mac}&serial=${serial}
initrd ${base}/images/rhel9/initrd.img
boot
```

rescriptum sert la réponse, pas le noyau — le netboot reste au serveur TFTP/HTTP que vous
faites déjà tourner.

### Ubuntu et cloud-init NoCloud

La source de données NoCloud de cloud-init récupère **deux** fichiers nommés depuis l'URL de
seed — `user-data` *et* `meta-data` — et ignore complètement la source de données si l'un des
deux manque. Comme ce serveur répond sur n'importe quel chemin, les deux requêtes recevraient
sinon le même document et l'installation ne démarrerait jamais.

Le dernier segment du chemin est disponible comme fait `file`, ce qui permet de les
distinguer avec un sélecteur :

```yaml
# answers/groups/ubuntu-web/ubuntu.yaml
match:
  file: "user-data"
  product: "PowerEdge R6*"
```

```yaml
# answers/groups/ubuntu-meta/ubuntu.yaml
match:
  file: "meta-data"

instance-id: iid-local01
```

Notez le slash final dans `s=http://SERVER:8000/ubuntu/` — cloud-init y accole le nom de
fichier.

NoCloud peut aussi développer `__dmi.chassis-serial-number__` dans l'URL de seed, ce qui met
l'identité de la machine dans le *chemin* plutôt que dans la query. Les segments de chemin
alimentent aussi la botte de foin, donc un document nommé d'après ce numéro de série résout
quand même.

## Choisir l'alias

| Segment d'URL | Sert les documents d'extension |
|---|---|
| `proxmox`, `pve`, `toml` | `.toml` |
| `debian`, `preseed` | `.preseed`, `.seed` |
| `rhel`, `centos`, `fedora`, `alma`, `rocky`, `kickstart`, `ks` | `.ks` |
| `ubuntu`, `autoinstall`, `cloudinit`, `nocloud`, `yaml`, `yml` | `.yaml`, `.yml` |
| `flatcar`, `coreos`, `ignition`, `ign` | `.ign`, `.json` |
| `suse`, `opensuse`, `autoyast` | `.autoyast`, `.xml` |
| `windows`, `unattend` | `.unattend`, `.xml` |
| `json` | `.json`, `.ign` |
| `xml`, `cfg`, `ipxe` | l'extension correspondante |

N'importe quel segment du chemin peut nommer l'alias, donc `/rhel/ks`, `/ks` et
`/provision/rhel/node.cfg` restreignent tous au kickstart. Une URL n'en nommant aucun —
`/answer` — ne contraint rien.

La table complète, et pourquoi `seed` n'est délibérément **pas** un alias, sont dans la
[référence des formats](./reference/formats.md).

## Ensuite

- [Comment une réponse est choisie](./answers/selection.md) — ce que le serveur fait de ce
  que l'URL vient de lui dire.
- [Un document par système d'exploitation](./answers/formats.md) — la même machine, en
  Proxmox et en Debian, en même temps.
