---
title: Pièges déjà rencontrés
description: Des choses qui ont coûté du temps une fois. Lire ceci coûte moins cher que les redécouvrir.
sidebar:
  label: Pièges
  order: 11
---

# Pièges déjà rencontrés

Chacun de ceux-ci a coûté du temps réel. Aucun n'est évident à la seule lecture du code.

## À l'exécution, pas à la compilation

**hyper panique si un timeout est défini sans timer.** `http1::Builder::header_read_timeout`
exige `.timer(TokioTimer::new())`. Omettez-le et **chaque** connexion panique à l'exécution —
cela ne casse pas la compilation. Les tests d'intégration l'ont attrapé ; des tests unitaires
n'auraient pas pu.

**`header_read_timeout` s'arrête à la fin des en-têtes.** hyper n'a pas de délai de lecture de
corps, donc un client qui promet un corps sans l'envoyer garerait une connexion indéfiniment.
Le `tokio::time::timeout` sur la connexion entière dans `connection()` est ce qui couvre cela.
**Les deux sont nécessaires ; aucun n'est redondant.**

**hyper émet les noms d'en-tête en minuscules.** C'est correct — ils sont insensibles à la
casse — donc affirmez sur une copie en minuscules. Voir `has_header` dans les tests
d'intégration.

## Performance

**`fs::metadata` par entrée de répertoire est un appel système `stat` chacun.**
`DirEntry::file_type()` revient gratuitement avec le `readdir` sur Unix ; seul un lien
symbolique a besoin du `stat` pour être résolu. Cela seul valait **65 % à 2 000 fichiers**,
avant que le cache ne soit ajouté.

**Éditer le *contenu* d'un fichier de groupe ne change aucun mtime de répertoire.** Seul
`RELOAD_BACKSTOP` (1 s) l'attrape, ce qui est pourquoi le filet n'est pas redondant avec la
vérification du mtime. Un test d'intégration le couvre.

**Un `Content-Length` aberrant doit être refusé depuis l'en-tête**, pas en laissant `Limited`
sauter après avoir tamponné un mégaoctet.

**Fermer sur un pair qui écrit encore jette la réponse qu'on vient d'écrire.** Le noyau
envoie un reset, et le reset détruit les octets non lus — le client voit donc une connexion
coupée, pas votre réponse. `shed()` avait exactement ce défaut : il écrivait son `503` et
fermait aussitôt, si bien que l'installateur à qui il essayait de dire *« réessaie »*
recevait un reset. Il draine maintenant brièvement d'abord, comme le faisait déjà le
`put()` de l'API d'administration. Un test au plafond de connexions l'épingle.

## Sélection et formats

**Un Mac qui édite le répertoire de réponses en SMB peut détourner la réponse d'une
machine.** macOS écrit un fichier AppleDouble `._<nom>` à côté d'un fichier dont le système
n'accepte pas les attributs étendus — `._98-fa-9b-50-d8-10.toml` a une extension *présente*
dans la liste, et la normalisation retire le `._` de tête : il revendique donc la même
identité que le vrai fichier, avec un contenu binaire. La machine qu'on configurait reçoit
une erreur d'analyse au lieu de sa réponse, et `check` fait échouer le *groupe* avec elle.
`.DS_Store` n'est inoffensif que par chance (son extension n'est pas dans la liste). Le store
fichiers ignore désormais toute entrée dont le nom commence par `.` ; trouvé sur un vrai NAS,
pas en lisant quoi que ce soit.

**Normaliser un motif de sélecteur retire `*` et `?`** à moins d'utiliser `normalize_pattern`
— ce qui transforme chaque glob en littéral, en silence.

**Dans un format texte, un placeholder à l'intérieur d'un commentaire reste un
placeholder.** `Kind::Text` est une chaîne opaque, donc la substitution parcourt tout le
document — un `{{ mac }}` écrit dans un commentaire `#` pour *expliquer* le templating doit
quand même se résoudre, et fait échouer `check` exactement comme un vrai. Trouvé en
ajoutant les exemples travaillés `.ipxe` et `.cfg`.

**Un GET n'a pas de corps, donc la botte de foin est vide.** Les valeurs de query et les
segments de chemin doivent l'alimenter aussi, sinon un document nommé d'après une MAC ne peut
jamais répondre à une récupération de preseed ou de kickstart.

**quick-xml émet les références d'entité comme leurs propres événements.** Les ignorer soude
les fragments de texte alentour : `1 &lt; 2 &amp; 3` revenait en `123`.

**Des frères XML répétés ne sont pas toujours une liste.** S'ils portent un attribut
discriminant, ce sont une collection indexée ; les traiter comme une liste remplaçait chaque
`<component>` d'un unattend.xml par celui que la surcouche mentionnait.

**Deux documents ayant le même radical ne sont pas des doublons.** Un `put` antérieur
supprimait les autres formats d'un radical pour éviter « deux réponses pour une machine ».
C'était le mauvais modèle : ce sont les réponses de cette machine pour deux **systèmes
d'exploitation**.

**Filtrer les endpoints sur l'extension, pas sur le `Kind`.** `.ks` et `.preseed` sont tous
deux `Kind::Text` ; filtrer par famille laisserait un preseed répondre à `/rhel/ks`.

**Un alias doit être assez spécifique pour que personne ne l'atteigne par accident.** `seed` a
été retiré comme alias d'endpoint : `s=http://server/seed/` est une URL de seed NoCloud
ordinaire, et elle sert du YAML.

## L'API d'administration

**Lire le corps de la requête avant de la rejeter.** Répondre et fermer pendant que le client
écrit encore lui vaut un `ECONNRESET` au lieu de la réponse. `put()` draine d'abord, puis
valide l'identifiant.

**Les réponses d'administration doivent définir `Connection: close`.** Sans cela, chaque client
de test attendait l'expiration de la connexion — la suite prenait 30 s au lieu de 0,4 s — et la
coupure finale arrivait parfois comme un reset plutôt qu'un EOF propre.

**Les identifiants deviennent des noms de fichiers.** `export` et le store fichiers construisent
des chemins à partir d'identifiants de machine et de noms de groupe, donc `valid_id` est imposé
à la frontière de l'API **et** dans les deux stores.

## Tests

**`cargo test` ne reconstruit pas `target/debug/rescriptum`.** Une vérification manuelle contre
un binaire périmé a un jour « reproduit » un bug déjà corrigé. Reconstruisez avant de triturer
le binaire à la main.

**Les tests d'invalidation de cache doivent partager une seule instance d'`Answers`.** Un test
qui en construit une nouvelle à chaque appel contourne complètement le cache et ne prouve
silencieusement rien.

**Affirmez sur des valeurs parsées, pas sur la mise en forme.** Remplacer une table par un
scalaire laisse la décoration d'origine de la clé, donc la sortie peut se lire `value= 3` — du
TOML valide, un texte différent.

**Un patch `python`/`sed` qui « réussit » peut n'avoir rien matché.** Deux éditions dans
l'histoire de ce projet n'ont silencieusement rien fait et n'ont été attrapées qu'en vérifiant
le nombre de tests ensuite. Vérifiez que l'ancien texte a été trouvé avant d'écrire.

## Empaqueter pour DSM

**musl 1.2 ne peut pas tourner sur les noyaux ARMv7 de Synology, et le symptôme ne nomme
rien.** Ces noyaux sont des 3.10 et répondent `EINVAL` aux appels *time64* ; musl ne se
replie sur les appels 32 bits que sur `ENOSYS`, donc `clock_gettime`, `clock_nanosleep` et le
futex temporisé échouent tous. Le binaire s'installe, répond à `--version`, puis panique à
`time.rs:131` avec `Os { code: 22, kind: InvalidInput }` dès qu'il veut un horodatage — ce
qui ressemble à un problème d'ABI ou de noyau trop vieux, et n'est ni l'un ni l'autre. La
cible armv7 est en glibc avec un plancher 2.17 pour cette raison ; les cibles 64 bits n'ont
pas le clivage time32/time64 et ne sont pas concernées. Prouvé par une sonde C de dix lignes
sur la machine, pas en lisant quoi que ce soit.

**`SYNOPKG_PKGDEST` vaut `/volume1/@appstore/<paquet>`, pas `/var/packages/<paquet>/target`.**
Le second est un lien vers le premier, donc `dirname "$SYNOPKG_PKGDEST"` donne
`/volume1/@appstore` et tout ce qu'on y accroche — `etc/`, `var/`, `shares/` — atterrit là où
rien ne lit. La racine du paquet est un chemin fixe. Ça coûte un service qui s'installe
parfaitement et ne démarre jamais, et un harnais sur faux arbre ne peut pas l'attraper : dans
un arbre qu'on a construit soi-même, `dirname` tombe juste par construction.

**`$SYNOPKG_TEMP_UPGRADE_FOLDER` survit à la mise à jour qui l'a créé.** Une installation
*neuve* qui le lit y trouve la configuration d'une installation que l'utilisateur a
supprimée, et la restaure en silence — jetons compris. La restauration doit exiger
`SYNOPKG_PKG_STATUS = UPGRADE`.

**`etc/` et `var/` survivent à une désinstallation.** Ce sont des liens vers
`/volume1/@appconf/<pkg>` et `/volume1/@appdata/<pkg>`, que DSM conserve. Le fichier
d'environnement, jetons inclus, reste donc sur le volume après la disparition du paquet — ce
que la documentation doit dire, et qui fait échouer le tour *suivant* d'un banc qui ne les
efface pas, pour des raisons appartenant au précédent.

**Un compte DSM portant le nom de l'utilisateur du paquet est détruit avec lui.** Le
`username` de `conf/privilege` crée un utilisateur système à l'installation ; un
administrateur du même nom est masqué par lui puis supprimé à la désinstallation.

**Le répertoire du pare-feu est `/usr/local/etc/services.d/`** — au pluriel. Le guide
développeur dit `service.d`, qui n'existe pas. Le worker `port-config` acquiert **après
`postinst`**, donc le port de l'assistant atteint bien l'entrée pare-feu dès l'installation.

**`port-config` et `usr-local-linker` acquièrent quand le paquet est *activé*,** pas quand
`postinst` tourne : vérifiés plus tôt, ils sont toujours absents.

**L'unité générée n'a pas de `Restart=`** — `Type=oneshot`, `RemainAfterExit=yes`,
`TimeoutStartSec=3600`. DSM ne relance pas le processus s'il meurt.

**`postinst` tourne aussi à une mise à jour, et il tourne *avant* `postupgrade`.** Donc
« le fichier d'environnement est absent » n'est pas la même question que « c'est une
installation neuve » : sur une mise à jour où `etc/` n'a pas survécu, y écrire les valeurs
par défaut détruit le port et les jetons de l'utilisateur avant que la restauration ne
tourne. `postinst` consulte `$SYNOPKG_TEMP_UPGRADE_FOLDER` avant de décider. Trouvé en
simulant ce cas précis, pas en lisant la séquence documentée.

**Les `preuninst`/`postuninst` de l'ancienne version tournent pendant une mise à jour.**
Tout ce qu'ils ont de destructeur tourne donc à chaque mise à jour — et le **premier `.spk`
publié** est celui dont les scripts de désinstallation tourneront pendant la première mise
à jour de tout le monde. Ils ne peuvent pas être corrigés après coup.

**`status` qui renvoie `1` veut dire « planté, pidfile resté »**, pas « arrêté ». Un paquet
proprement arrêté, c'est `3`. Renvoyer `1` dit à Package Center que le service est mort.

**`prestart` tourne au boot**, et DSM l'appelle que vous l'ayez écrit ou non —
`precheckstartstop` vaut `"yes"` par défaut. Un `case` qui sort non-zéro sur un verbe
inconnu empêche le paquet de démarrer après un reboot pour toujours, avec un symptôme
(« marche à la main, jamais après un reboot ») qui ressemble à tout sauf à un bras de `case`
manquant.

**Les scripts de cycle de vie ne sont pas root.** `run-as: package` les gouverne, pas
seulement le service — donc un chown hors de l'arbre du paquet, ou `synopkghelper`, échoue,
possiblement en silence.

**`data-share` tourne au *démarrage* du paquet, pas à l'installation**, donc rien dans
`postinst` ne peut supposer que le dossier partagé existe. Et un nom d'utilisateur qui ne
correspond pas à sa liste de permissions crée le partage et l'accorde à personne, sans un
mot.

**Une strophe logrotate sans `copytruncate` arrête silencieusement la journalisation** :
`log::init` ouvre le fichier une fois et ne le rouvre jamais, donc une rotation déplace
l'inode sous un serveur qui continue d'écrire dans un fichier sans nom.

**Un `.spk` dont le tar externe est gzippé est rejeté** avec « invalid file format » et rien
de plus. Idem pour un qui embarque des membres `._` de macOS. `check-spk.sh` vérifie les
deux.

## Changements de comportement à retenir

**Les documents de réponse doivent maintenant être valides.** Avant la fusion, ils étaient
servis comme des octets opaques, donc un document malformé atteignait l'installateur ;
maintenant c'est un `500` avec l'erreur de parsing dans le log. C'est le meilleur échec, mais
*c'est* un changement de comportement — des fixtures écrites en pseudo-YAML ont cessé de
fonctionner à ce moment-là.

**`{{ machine }}` n'est lié que si un document machine a matché.** Une machine revendiquée par
les `members` d'un groupe, sans document à elle, se résout avec `machine: None` — donc
`{{ machine }}` dans un groupe échoue pour exactement les membres qu'il devait couvrir.
Utilisez là un fait de requête comme `{{ mac }}`.
