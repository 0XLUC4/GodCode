# GodCode — Plan

Objectif : un IDE universel, moderne, rapide, personnalisable, pour un maximum de langages,
frameworks et outils. Local d'abord : pas de compte, pas de serveur, pas de télémétrie.
L'IA vient en dernier : l'éditeur doit d'abord tenir debout seul.

## Principes

- **Rust pur**, un seul exécutable, démarrage instantané. Chaque dépendance doit se justifier.
- **`crates/core` ne dépend jamais de l'interface** : fonctions pures, testées.
- **Protocoles plutôt que code par langage** : LSP pour l'intelligence, DAP pour le débogage.
  C'est ce qui permet « tous les langages » sans écrire 80 analyseurs.
- Chaque fonction livrée est testée et vue tourner. Une ligne du plan n'est cochée qu'à ce prix.

## Architecture

```
crates/
  core/  godcode-core   coloration, éditions, recherche, format, outline, markdown. Sans UI.
  app/   godcode        application eframe/egui : éditeur, panneaux, terminal, git.
  lsp/   (phase 2)      client Language Server Protocol : JSON-RPC sur stdio, hors thread UI.
  dap/   (phase 4)      client Debug Adapter Protocol.
```

Limite connue : l'éditeur repose sur le champ de texte d'egui (un seul curseur, texte mis en
page en entier). Multi-curseurs, repli de code et fichiers de plus de 2 Mo demandent un
widget d'édition maison : c'est le chantier de la phase 3, à ne pas bricoler avant.

## État

Légende : ✅ fait · 🔨 en cours · ⏳ prévu · ❌ hors périmètre pour l'instant.

### Phase 0 — socle (✅ livré)

Onglets, explorateur, recherche dans les fichiers, terminal intégré, Git (statut, commit,
pull/push, lignes modifiées dans la marge), palette de commandes, complétion par mots et
snippets, vérification des crochets, compilateurs à la sauvegarde (Rust, Python, JS, Go),
thèmes sombre/clair, zoom.

### Phase 1 — éditeur professionnel (🔨)

| Fonction | État |
|---|---|
| Reformat Code (HTML, CSS, JS/TS, JSON, accolades ; rustfmt/gofmt) | ✅ |
| Soft wrap à la demande | ✅ |
| Search Everywhere, fichiers récents, Go to Declaration, Find Usages, Back/Forward | ✅ |
| Structure, TODO, fil d'Ariane, guides d'indentation, occurrences du mot | ✅ |
| Fermeture auto des crochets et des balises, Extend Selection, Join Lines, casse | ✅ |
| Icônes de fichiers par langage | ✅ |
| Aperçu Markdown avec défilement synchronisé | ✅ |
| ~75 langages colorés, langages imbriqués (JS/CSS dans HTML, blocs de code Markdown) | ✅ |
| Recherche par expression régulière et mot entier | ✅ |
| Restauration de session, sauvegarde automatique | ✅ |
| Mini-map, sticky scroll | ✅ |
| Diff côte à côte avec le dernier commit, Git blame en fin de ligne, branches (changer, créer) | ✅ |
| Rechargement des fichiers modifiés hors de l'éditeur | ✅ |
| Split vertical (deux éditeurs côte à côte) | ✅ |
| Onglets épinglés (clic droit sur l'onglet) | ✅ |
| Split horizontal (`Split Editor Down`) | ✅ |
| Même fichier dans deux vues (`Split Editor: Same File`, ou `Ctrl+\` avec un seul fichier ouvert) | ✅ |
| Historique local (une version à chaque sauvegarde, 40 gardées) | ✅ |
| Snippets personnalisables (`Edit Snippets` : un fichier JSON par langage) | ✅ |
| Raccourcis configurables (`Edit Keyboard Shortcuts` : nom de la commande → raccourci) | ✅ |

### Phase 2 — moteur de langages (🔨)

Client LSP générique (`crates/lsp`) : un serveur par langage et par projet, lancé à la
première ouverture d'un fichier, lectures et écritures hors du thread de l'interface.
La barre d'état dit quel serveur tourne ; un clic dit comment l'installer quand il manque.

| Fonction | État |
|---|---|
| Cycle de vie : initialize, didOpen / didChange / didSave / didClose, arrêt | ✅ |
| Détection des serveurs installés, raison affichée quand un serveur refuse de démarrer | ✅ |
| Diagnostics en temps réel (soulignés, marge, panneau Problems) | ✅ |
| Documentation et types sous le curseur (`Ctrl+Q`) | ✅ |
| Complétion sémantique dans la liste existante | ✅ |
| Aller à la définition entre fichiers (`F12`, `Ctrl+Clic`) | ✅ |
| Références entre fichiers (`Alt+F7`), listées dans le panneau Search | ✅ |
| Renommage sûr dans tout le projet (`F2`), fichiers fermés compris | ✅ |
| Quick Fix (`Alt+Entrée`) | ✅ |
| Formatage par le serveur (`Ctrl+Alt+L`), sinon formateur intégré | ✅ |
| Documentation au survol de la souris | ✅ |
| Signature des fonctions pendant la frappe, argument en cours souligné | ✅ |
| Indices en ligne (inlay hints) : types, noms d'arguments, types de retour | ✅ dans le nouveau champ (phase 3) |
| Implémentations (`Ctrl+Alt+B`), symboles du projet (`#nom` dans Search Everywhere) | ✅ |
| Hiérarchie d'appels : appelants (`Ctrl+Alt+H`) et appels faits, un niveau, listés dans le panneau Search | ✅ |
| Actions qui passent par une commande du serveur (refactorings, déplacement vers un nouveau fichier), import manquant ajouté par Quick Fix | ✅ |
| Import ajouté en acceptant une complétion | ✅ |
| Synchronisation incrémentale (seul ce qui diffère est envoyé ; texte entier si le serveur ne sait pas) | ✅ |

21 langages ont un serveur connu : Rust, TypeScript/JavaScript, Python, Go, C/C++, HTML,
CSS, JSON, Shell, YAML, TOML, Lua, Zig, Dart, PHP, Ruby, Java, Kotlin, C#, Haskell, Elixir.
Vérifié de bout en bout avec typescript-language-server ; les autres parlent le même
protocole mais n'ont pas été lancés ici. `GODCODE_LSP_LOG=1` affiche les messages échangés.
`cargo test -p godcode-lsp -- --ignored` rejoue la vérification contre le vrai serveur
(survol, implémentations, symboles, synchronisation incrémentale, hiérarchie d'appels, Quick Fix, import par complétion, indices en ligne). Il lui faut
`typescript@5` : TypeScript 7 ne fournit plus le `tsserver` qu'il pilote.

### Phase 3 — widget d'édition maison (🔨)

Fait sans attendre le widget : **multi-curseurs** (`Ctrl+Alt+Haut/Bas`, `Alt+Clic`) — frappe,
effacement et déplacement à tous les curseurs, sur le champ de texte actuel.

Le widget maison a commencé (`crates/app/src/field.rs`) : un champ de texte qui ne met en
page que les lignes visibles. Il garde son curseur et son historique là où le champ d'egui
les garde, donc tout le reste de l'éditeur (marques, gouttière, complétion, multi-curseurs,
commandes) marche avec les deux. Il s'active par `Toggle Fast Rendering (Experimental)`
(menu View) et reste éteint par défaut.

| Tranche | État |
|---|---|
| Rendu virtualisé : frappe, sélection clavier et souris, copier/couper/coller, annuler/rétablir, une ligne = une rangée | ✅ derrière le réglage |
| Les tests de l'éditeur rejoués sur le nouveau champ | ✅ |
| Retour à la ligne (soft wrap) dans le nouveau champ : aujourd'hui l'ancien champ reprend la main | ⏳ |
| Saisie par IME (chinois, japonais…), défilement automatique pendant une sélection à la souris | ⏳ |
| Fichiers de plus de 2 Mo : le champ le permet, le reste de l'app relit encore tout le texte à chaque frappe | ⏳ |
| Repli de code : flèche près du numéro de ligne, `Fold / Unfold Block`, `Unfold All` ; blocs trouvés par l'indentation ; un bloc s'ouvre quand le curseur y entre | ✅ derrière le réglage |
| Replis gardés d'une session à l'autre, repli par accolades ou par le serveur de langage | ⏳ |
| Indices en ligne (inlay hints) : écrits dans la ligne sans faire partie du texte, redemandés à chaque changement ; le serveur TypeScript est réglé pour en donner, rust-analyzer et clangd en donnent d'eux-mêmes (non lancés ici) | ✅ derrière le réglage |
| Réglage pour couper les indices, indices demandés pour les seules lignes visibles (aujourd'hui : tout le fichier) | ⏳ |
| Sélection en colonnes, modes Vim/Emacs | ⏳ |
| Bascule par défaut, puis retrait de l'ancien champ | ⏳ |

### Phase 4 — exécution, tests, débogage (⏳)

Fait : `Run Task…` (`Ctrl+Shift+R`) liste les scripts npm, les commandes cargo et go, les
cibles du Makefile. Fait : `Run Test at Caret` (`Ctrl+Shift+F10`) lance le test sous le curseur (Rust, Python,
Go, JS/TS). Fait : `Run Tests` (`Ctrl+Shift+T`) lance la commande de test du projet (cargo, npm, go,
pytest, maven, gradle) dans le terminal.

Fait : **débogueur** (`crates/lsp/src/dap.rs`, client DAP) — points d'arrêt (clic sur le
numéro de ligne ou `F9`), `Ctrl+F5` lance, `F5` continue, `F10` / `F11` / `Shift+F11` pas à
pas, `Shift+F5` arrête ; panneau Debug avec pile d'appels, variables et sortie du
programme. Vérifié de bout en bout avec Python (debugpy). Les autres langages se règlent
dans `Edit Debug Configurations` (adaptateur + réglages de lancement, `${file}` et
`${folder}`) : mécanisme fait, aucun autre adaptateur vérifié ici.

Fait : panneau **Tests** — les tests du fichier ouvert, un clic lance celui choisi. Reste : configurations d'exécution, tâches, explorateur de tests (cargo test, pytest, vitest, go
test, JUnit), couverture ; puis client DAP : points d'arrêt, pas à pas, variables, pile
d'appels, watch.

### Phase 5 — Git complet et projets (⏳)

Fait : branches, blame, diff avec le dernier commit, journal des commits avec le diff de
chacun, stash et stash pop. merge et rebase d'une branche, conflits réglés au
curseur (garder le mien, le leur, les deux). Reste : graphe des branches, vue de fusion
à trois colonnes ; monorepos, variables d'environnement. Fait : `New Project…` crée un projet depuis un
modèle (page web, Node/TypeScript, Python, Rust).

### Phase 6 — outils (⏳)

Fait : client HTTP — fichiers `.http`, `F5` envoie la requête sous le curseur, réponse
(JSON mis en forme) dans le panneau Response ; passe par `curl`. Reste : GraphQL,
WebSocket, collections.

Fait : **aperçu web en direct** (`Live Preview in Browser`) — le dossier est servi sur
127.0.0.1 uniquement, la page se recharge seule à chaque sauvegarde. Reste : aperçu
intégré à la fenêtre, inspecteur, bases de données (SQLite et
PostgreSQL d'abord), Docker.

### Phase 7 — personnalisation et extensions (⏳)

Fait : 6 thèmes (GodCode Dark/Light, Darcula, Monokai, Solarized Light, High Contrast),
mode Zen (`Ctrl+Alt+Z`). Reste : créateur de thèmes, polices, dispositions enregistrables,
langues d'interface ;
API de plugins (WASM isolé) une fois le cœur stable.

### Phase 8 — IA (⏳, volontairement en dernier)

Assistant contextuel, complétion multiligne, agents avec validation humaine, modèles
locaux, MCP.

### Hors périmètre tant que le reste n'est pas solide (❌)

Collaboration temps réel, marketplace, SSO, intégrations cloud (AWS/Azure/GCP), Kubernetes,
développement distant, time-travel debugging. Ce sont des produits à part entière.
