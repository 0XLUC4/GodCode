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

L'éditeur repose sur un champ de texte maison (`crates/app/src/field.rs`) qui ne met en page
que les rangées visibles. Limite connue : le texte reste une seule `String`, copiée en entier
dans l'historique d'annulation ; au-delà de 2 Mo un fichier s'ouvre en texte brut (voir phase 3).

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

### Phase 3 — widget d'édition maison (✅ livré, suites ⏳)

Le champ de texte d'egui est retiré : `crates/app/src/field.rs` est le seul champ. Il ne met
en page que les rangées visibles, et garde son curseur et son historique là où le champ
d'egui les gardait (l'état rangé sous son id), ce que lisent les commandes de l'éditeur.

| Tranche | État |
|---|---|
| Rendu virtualisé : frappe, sélection clavier et souris, copier/couper/coller, annuler/rétablir | ✅ |
| Multi-curseurs (`Ctrl+Alt+Haut/Bas`, `Alt+Clic`) : frappe, effacement et déplacement à tous les curseurs | ✅ |
| Retour à la ligne (soft wrap, `Alt+Z`) : rangées coupées aux espaces, comptées sans mettre le texte en page ; haut/bas vont de rangée en rangée | ✅ |
| Saisie par IME : texte en cours de composition, fenêtre de l'IME placée au curseur | ✅ testé par événements simulés, pas avec un vrai IME |
| Défilement automatique pendant une sélection à la souris | ✅ |
| Repli de code : flèche près du numéro de ligne, `Fold / Unfold Block`, `Unfold All` ; un bloc est ce que tient un crochet laissé ouvert (hors chaînes et commentaires), sinon ce qui est indenté plus loin ; il s'ouvre quand le curseur y entre ; les replis sont gardés d'une session à l'autre | ✅ |
| Repli donné par le serveur de langage (`textDocument/foldingRange`) | ⏳ |
| Indices en ligne (inlay hints) : écrits dans la ligne sans faire partie du texte ; demandés pour les lignes autour de la vue (100 au-dessus, 300 en dessous), à chaque changement et quand la vue en sort ; `Toggle Inlay Hints` les coupe. Le serveur TypeScript est réglé pour en donner, rust-analyzer et clangd en donnent d'eux-mêmes (non lancés ici) | ✅ |
| Indices suivis pendant la frappe (aujourd'hui : les anciens restent un instant, parfois décalés, jusqu'à la réponse du serveur) | ⏳ |
| Sélection en colonnes : `Alt+Maj+glisser` ou `Ctrl+Alt+Maj+flèches` ; ce qui est tapé remplace chaque ligne du rectangle, puis chaque ligne garde un curseur | ✅ |
| Touches Vim (`Switch Keys`) : modes normal, insertion, visuel et visuel ligne ; déplacements `h j k l w b e 0 ^ $ gg G f F t T ; ,` avec compte ; `d c y` avec déplacement, doublés ou sur un objet de texte (`iw aw`, `i( a(`, `i{`, `i[`, `i<`, `i"`, `i'`) ; `x D C p P r J o O i a I A u Ctrl+R /` ; `.` refait le dernier changement, texte tapé compris ; mode affiché dans la barre d'état, curseur bloc. Moteur pur et testé : `crates/core/src/modal.rs` | ✅ |
| Touches Emacs : `Ctrl+F B N P A E`, `Alt+F B`, `Ctrl+D K Y W`, `Alt+W D`, marque `Ctrl+Espace`, `Ctrl+G`, `Ctrl+/`, `Ctrl+S` cherche, `Ctrl+X Ctrl+S` enregistre, `Ctrl+X Ctrl+F` ouvre | ✅ |
| Vim : registres nommés, marques, macros, `:`, `%`, `*`, compte devant `.` | ⏳ |
| Fichiers jusqu'à 64 Mo (2 Mo avant). Au repos, rien n'est relu (70 µs par image à 50 Mo) ; le texte n'est relu qu'après un changement. Au-delà de 2 Mo : texte brut, sans couleurs, serveur de langage, plan, complétion par mots ni historique local. Mesuré : une frappe coûte 16 ms à 10 Mo, 80 ms à 50 Mo (`cargo test --release -p godcode typing_in_a_big -- --ignored --nocapture`) | ✅ |
| Gros fichiers : frappe en temps constant (corde ou tampon à trou au lieu d'une `String`), couleurs refaites pour les seules lignes changées, annulation par différences au lieu de copies entières (aujourd'hui : copies plafonnées à 256 Mo) | ⏳ |

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
