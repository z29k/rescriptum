---
title: Ce qu'est rescriptum
description: Une machine décrit son matériel ; rescriptum compose la réponse pour elle et la renvoie. Le problème que cela pose, et les quatre idées qui le résolvent.
sidebar:
  label: Ce qu'est rescriptum
  order: 0
---

# Ce qu'est rescriptum

*rescriptum* — en droit romain, la **réponse écrite d'une autorité à une question soulevée
par un cas particulier**. Vous exposiez votre situation ; vous receviez un document rédigé
pour elle.

rescriptum sert la **configuration d'installation qu'une machine réclame en s'installant**,
et la compose machine par machine à partir des couches que toute une baie partage. Un seul
serveur répond à tous les installateurs que vous faites tourner. Servir un fichier est
facile — décider lequel est tout le travail.

## Le problème

**Tout installateur automatisé va chercher sa configuration en HTTP, et chaque machine en a
besoin d'une différente.** L'URL est gravée dans le média, donc elle est identique sur
chaque machine — un serveur de fichiers statique ne peut donc pas les servir. Les
installateurs demandent de deux façons.

### Ils POSTent ce qu'ils ont trouvé

Depuis Proxmox VE 8.2, un installateur préparé avec `--fetch-from http` **POST une
description JSON du matériel qu'il a trouvé** — cartes réseau et adresses MAC, disques,
DMI — et attend le fichier de réponse dans le corps de la réponse HTTP :

```json
{
  "network_interfaces": [{ "mac": "98:fa:9b:50:d8:10", "link": "up" }],
  "dmi": { "system": { "serial": "7ABC123", "product": "PowerEdge R620" } }
}
```

**La réponse dépend de la requête.** Un serveur de fichiers statique ne sait pas faire ça :
il a une réponse pour une URL, et l'URL est gravée dans l'ISO — identique sur chaque
machine.

### Ils GETent avec leur identité dans la query string

Tout le reste a la forme inverse. Un kickstart, un preseed, un autoinstall Ubuntu, une
configuration Ignition, un profil AutoYaST sont *récupérés*, avec l'identité de la machine
dans la query string, parce qu'iPXE la substitue dans l'URL qu'on lui a dit d'aller
chercher :

```
GET /rhel/ks?serial=7ABC123&mac=98:fa:9b:50:d8:10
```

Dans les deux cas, la réponse doit être choisie — et le plus souvent assemblée — machine
par machine.

## Les quatre idées

**1. L'endpoint déclare le format.** Un client kickstart veut du kickstart et
s'étranglerait avec du TOML. Donc `/rhel/ks` sert des documents `.ks` et rien d'autre,
`/proxmox/answer` sert du `.toml`, `/ubuntu/` sert du YAML. La conséquence qui fait
comprendre le modèle : la réponse d'une machine est spécifique au système d'exploitation
auquel elle est destinée, donc `98fa9b50d810.toml` n'est pas « cette machine » mais
*« cette machine en tant que Proxmox »* — et `98fa9b50d810.preseed` est le même matériel en
tant que Debian. Les deux existent en même temps.
→ [Un document par système d'exploitation](./answers/formats.md)

**2. Une machine est revendiquée, pas cherchée.** Nommez un document d'après la MAC et il
gagne. Ou listez la machine dans les `members` d'un groupe. Ou écrivez un bloc `[match]` et
laissez la machine être revendiquée pour ce qu'elle *est* — un Dell R620 dont le numéro de
série commence par `7ABC`. La résolution est déterministe : nommer bat matcher, plus de
critères bat moins, les égalités se départagent sur le nom trié.
→ [Comment une réponse est choisie](./answers/selection.md)

**3. Les réponses se composent.** Une baie de machines partage tout sauf ses adresses MAC.
Mettez la partie commune dans un groupe ; une machine qui diffère reçoit un fichier
contenant **seulement la différence**. Les formats structurés fusionnent vraiment — les
maps clé par clé, les tableaux remplacés pour qu'une liste puisse encore être raccourcie.
Ajoutez des placeholders `{{ serial }}` et un seul fichier de groupe couvre cinq cents
machines.
→ [Groupes et fusion](./answers/grouping.md) · [Templating](./answers/templating.md)

**4. Ce qui est servi est relisible avant d'être servi.** La fusion crée un document que
personne n'a jamais écrit, et une mauvaise fusion se manifeste par une installation
automatisée ratée à 3 h du matin. `rescriptum render` affiche exactement ce qu'une machine
donnée recevrait ; `rescriptum check` rend tout et signale ce qui casse, en appelant le
validateur de l'installateur lui-même quand il est dans le PATH.
→ [Valider ce qui sera servi](./answers/validating.md)

## Ce que ce n'est pas

- **Pas un serveur PXE/TFTP/DHCP.** Il répond à une seule question — *quelle configuration
  reçoit cette machine ?* — et laisse le netboot à ce que vous faites déjà tourner.
- **Pas un validateur de schéma.** Il prouve que vos documents sont bien formés et
  fusionnent proprement. Savoir si le résultat est du *Proxmox* valide est le travail de
  `proxmox-auto-install-assistant`, et `check` l'appellera s'il est installé.
- **Pas un système de gestion de configuration.** Il remet un document au moment de
  l'installation et n'a plus rien à voir avec la machine ensuite.

## Deux réalités de déploiement

Les deux sont réelles, et la conception doit satisfaire les deux :

- **Un Synology DS416j** — ARMv7, 512 Mo, DSM 7, pas de Docker. La motivation d'origine, et
  la raison pour laquelle c'est un binaire statique unique sans runtime ni interpréteur.
- **Un hôte de datacenter** encaissant une rafale de provisioning, avec un fichier de
  réponse par machine. La raison pour laquelle il est asynchrone, borne sa propre
  concurrence, et met en cache le listing du répertoire au lieu de le parcourir à chaque
  requête.

À 2 000 machines, une baie servie depuis un seul groupe rend **13 000 requêtes/seconde**
sans rien parser par requête — le groupement est le chemin rapide, pas seulement le plus
propre.

## Où aller ensuite

- [Installation](./install.md) — mettre le binaire en route.
- [Servir sa première réponse](./quickstart.md) — de bout en bout en cinq minutes.
- [Préparer les médias d'installation](./iso.md) — l'URL à graver dans chaque ISO.
- [Écrire des réponses](./answers/index.md) — sélection, formats, groupes, templating.
- [L'exploiter](./operations/index.md) — déploiement, sécurité, stockage, dépannage.

Vous travaillez *sur* rescriptum plutôt qu'avec ? L'espace
[Développement](../development/index.md) est l'autre moitié de ce site.
