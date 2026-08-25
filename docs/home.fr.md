# rescriptum

**Un serveur HTTP pour compiler et rendre les fichiers de configuration des installations
automatisées d'OS.** Il répond à n'importe quel installateur automatisé : Proxmox, Debian,
RHEL, Ubuntu, Flatcar, SUSE, Windows. Il reconnaît la machine à son adresse MAC, son numéro
de série ou son inventaire matériel, empile les couches qui la concernent et renvoie le
résultat. Ce qu'une machine s'apprête à recevoir, vous pouvez le lire avant de l'allumer.

```console
$ RESCRIPTUM_ANSWERS_DIR=/srv/answers rescriptum
2026-08-22T18:00:00Z - rescriptum 0.1.0 listening on 0.0.0.0:8000 — store=files:/srv/answers workers=8 max_conn=2048 timeout=10s
2026-08-22T18:00:04Z 10.0.0.42:51234 POST /answer body=1876 200 format=toml machine=98-fa-9b-50-d8-10 group=rack-a bytes=412
```

Un binaire statique, sans runtime, sans conteneur — aussi à l'aise sur un NAS ARM de
512 Mo que sur un hôte de datacenter encaissant une rafale de provisioning.

## Commencer ici

- **[Ce qu'est rescriptum](./guide/index.md)** — le problème qu'il résout et la forme de
  la solution, en cinq minutes.
- **[Installation](./guide/install.md)** puis **[servir sa première réponse](./guide/quickstart.md)** —
  du binaire téléchargé à une machine qui reçoit sa propre configuration.
- **[Préparer les médias d'installation](./guide/iso.md)** — l'URL à graver dans chaque
  ISO, par système d'exploitation.

## Écrire des réponses

- **[Comment une réponse est choisie](./guide/answers/selection.md)** — par nom, par liste
  de membres, ou par ce que la machine *est*.
- **[Un document par système d'exploitation](./guide/answers/formats.md)** — l'extension
  est le format, l'endpoint choisit entre eux.
- **[Groupes et fusion](./guide/answers/grouping.md)** — une baie partage un fichier ; une
  machine qui diffère ne porte que sa différence.
- **[Templating](./guide/answers/templating.md)** — `{{ serial }}` dans un groupe couvre
  cinq cents machines.
- **[Valider ce qui sera servi](./guide/answers/validating.md)** — `render` et `check`,
  parce qu'une réponse fusionnée est un document que personne n'a jamais écrit.

## L'exploiter

- **[Déploiement](./guide/operations/deployment.md)** et
  **[Synology DSM 7](./guide/operations/synology.md)**.
- **[Sécurité](./guide/operations/security.md)** — ce que les jetons protègent, et ce
  qu'ils ne protègent pas.
- **[Le store SQLite](./guide/operations/sqlite.md)** et
  **[l'API d'administration](./guide/operations/admin-api.md)** — pour un parc administré
  par outillage plutôt qu'à la main.
- **[Dépannage](./guide/operations/troubleshooting.md)** — la ligne de log est tout le
  diagnostic disponible.

Les tables exhaustives vivent dans la [Référence](./guide/reference/index.md) : chaque
[variable d'environnement](./guide/reference/configuration.md), la
[surface HTTP](./guide/reference/endpoints.md), les
[tables de formats et d'alias](./guide/reference/formats.md), et la
[ligne de commande](./guide/reference/cli.md).

## Travailler sur rescriptum

L'espace [Développement](./development/index.md) est l'autre moitié de ce site : les
[contraintes](./development/constraints.md) qui façonnent le code et pourquoi elles ne
sont pas négociables, le [cycle de vie d'une requête](./development/request-lifecycle.md),
les internes de la [sélection](./development/selection.md), des
[formats](./development/formats.md) et des [stores](./development/stores.md), comment les
[tests](./development/testing.md) sont organisés, et comment une
[release](./development/releasing.md) est faite.
