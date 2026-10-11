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
que les rangées visibles. Limite connue : le texte reste une seule `String` ;
au-delà de 2 Mo un fichier s'ouvre en texte brut (voir phase 3).

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
| Icônes de fichiers par langage : le logo du langage dans sa couleur pour une cinquantaine de types (police devicon, licence MIT, embarquée : +1,2 Mo), des lettres pour ceux qui n'en ont pas | ✅ |
| Aperçu Markdown avec défilement synchronisé | ✅ |
| ~75 langages colorés, langages imbriqués (JS/CSS dans HTML, blocs de code Markdown) | ✅ |
| Recherche par expression régulière et mot entier | ✅ |
| Restauration de session, sauvegarde automatique | ✅ |
| `Toggle Format on Save` : un fichier enregistré à la main est d'abord mis en forme (formateur du langage s'il est installé, sinon l'intégré) ; pas à la sauvegarde automatique, ni par le serveur de langage | ✅ |
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
| Mise en forme pendant la frappe : Entrée indente après `{ [ ( :` (même suivis d'un commentaire), en HTML/XML après une balise ouvrante, et en Lua, Ruby, shell et Elixir après le mot qui ouvre un bloc (`then`, `do`, `def`…) ; entre `<body>` et `</body>` comme entre deux crochets, ce qui ferme passe sur sa ligne ; un `} ] )` ou un `end` / `fi` / `done` tapé en début de ligne revient d'un niveau | ✅ |
| Balises : `>` écrit la balise fermante sauf si une fermante l'attend déjà plus bas ; `</` écrit le nom de l'élément encore ouvert, sous sa balise ouvrante ; renommer une balise renomme sa jumelle (ouvrante ou fermante) au fil de la frappe | ✅ |
| Une couleur écrite dans une page, une feuille de style ou un script (`#rgb`, `#rrggbb`, avec ou sans opacité, `rgb()` / `rgba()`) est soulignée de sa propre couleur. Pas encore : couleurs nommées, `hsl()`, sélecteur de couleur | ✅ |
| Complétion web : attributs dans une balise (ceux de l'élément puis les communs, `class=""` avec le curseur entre les guillemets) et valeurs de `type`, `target`, `rel`, `method`… ; propriétés CSS et leurs valeurs courantes, dans un fichier `.css`, dans `<style>` et dans l'attribut `style` ; les noms à tiret (`font-size`, `aria-label`) sont suivis d'un bout à l'autre | ✅ |
| Abréviations à la Emmet, avec Tab. HTML : `ul>li.item$*3`, `#id`, `.classe`, `[attributs]`, `{texte}`, `>` `+` `*n`, groupes `( )`, remontée `^`, `!` pour une page entière. CSS, dans un bloc : `m10-20` → `margin: 10px 20px;`, `w100p`, `fz1.5r`, `c#333`, `df`, `jcsb`, `posa`… (`crates/core/src/emmet.rs`). Les abréviations CSS marchent aussi dans `<style>` | ✅ |

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
| Repli donné par le serveur de langage (`textDocument/foldingRange`) : quand il en donne, ce sont ses blocs qui se replient (commentaires, imports, régions compris) ; ils suivent la frappe jusqu'à sa réponse suivante | ✅ |
| Indices en ligne (inlay hints) : écrits dans la ligne sans faire partie du texte ; demandés pour les lignes autour de la vue (100 au-dessus, 300 en dessous), à chaque changement et quand la vue en sort ; `Toggle Inlay Hints` les coupe. Le serveur TypeScript est réglé pour en donner, rust-analyzer et clangd en donnent d'eux-mêmes (non lancés ici) | ✅ |
| Indices suivis pendant la frappe : ceux d'après le curseur glissent avec ce qui est tapé, ceux des lignes du dessous montent et descendent, jusqu'à la réponse du serveur ; ceux d'une ligne coupée ou jointe s'effacent en l'attendant | ✅ |
| Sélection en colonnes : `Alt+Maj+glisser` ou `Ctrl+Alt+Maj+flèches` ; ce qui est tapé remplace chaque ligne du rectangle, puis chaque ligne garde un curseur | ✅ |
| Touches Vim (`Switch Keys`) : modes normal, insertion, visuel et visuel ligne ; déplacements `h j k l w b e 0 ^ $ gg G f F t T ; ,` avec compte ; `d c y` avec déplacement, doublés ou sur un objet de texte (`iw aw`, `i( a(`, `i{`, `i[`, `i<`, `i"`, `i'`) ; `x D C p P r J ~ >> << o O i a I A u Ctrl+R /`, `:w` `:q` `:wq` `:42`, `%` va au crochet jumeau, `*` et `#` au même mot ; `.` refait le dernier changement, texte tapé compris ; mode affiché dans la barre d'état, curseur bloc. Moteur pur et testé : `crates/core/src/modal.rs` | ✅ |
| Touches Emacs : `Ctrl+F B N P A E`, `Alt+F B`, `Ctrl+D K Y W`, `Alt+W D`, marque `Ctrl+Espace`, `Ctrl+G`, `Ctrl+/`, `Ctrl+S` cherche, `Ctrl+X Ctrl+S` enregistre, `Ctrl+X Ctrl+F` ouvre | ✅ |
| Vim : registres nommés (`"a`–`"z`, majuscule pour ajouter, `"0`, `"_`), macros (`q{a-z}`, `@{a-z}`, `@@`, compte), `recording @a` dans la barre d'état ; une macro s'arrête à une touche qui demande quelque chose à l'éditeur (annuler, chercher, enregistrer) | ✅ |
| Vim : marques (`m{a-z}`, `'a`, `` `a ``, `''`, après un opérateur, propres à chaque onglet), `:s` avec plage, drapeaux `g` `i` et groupes (expressions de la bibliothèque `regex` : groupes `( )`), `:N,Md`, `:$`, `&`, compte devant `.`, registres `"1`–`"9` et `"-` | ✅ |
| Vim : marques de fichier `A`–`Z`, liste des sauts (`Ctrl-O`), `:g`, adresses relatives (`:.+2`), drapeau `c` de `:s` | ⏳ |
| Fichiers jusqu'à 64 Mo (2 Mo avant). Au-delà de 2 Mo : texte brut, sans couleurs, serveur de langage, plan, complétion par mots ni historique local. Au repos rien n'est relu ; pendant la frappe, seules les lignes touchées le sont. Mesuré à 50 Mo : une frappe en fin de fichier coûte 50 µs, 21 ms tout au début (le texte est une `String` : ce qui suit le curseur est déplacé) ; 82 ms avant (`cargo test --release -p godcode typing_in_a_big -- --ignored --nocapture`) | ✅ |
| Historique d'annulation par différences : seul ce qui change est gardé (64 Mo au plus), donc autant de niveaux dans un gros fichier que dans un petit ; une frappe groupée par pauses d'une seconde ; un changement fait ailleurs (commande, Vim, serveur de langage) est trouvé par comparaison | ✅ |
| Gros fichiers : frappe en temps constant partout (corde ou tampon à trou au lieu d'une `String`) ; Entrée, Tab et les touches Vim relisent encore tout le texte ; couleurs refaites pour les seules lignes changées | ⏳ |

### Phase 4 — exécution, tests, débogage (🔨)

| Fonction | État |
|---|---|
| `Run File` (`F5`) lance le fichier en avant dans le terminal | ✅ |
| `Run Task…` (`Ctrl+Shift+R`) : scripts npm, commandes cargo et go, cibles du Makefile | ✅ |
| Configurations d'exécution du projet : `Edit Run Configurations` ouvre `.godcode/run.json` (nom → commande, avec `${file}`, `${fileName}`, `${folder}`) ; elles viennent en tête de `Run Task…` ; `Run Last Task` (`Ctrl+Alt+R`) relance la dernière | ✅ |
| `Run Tests` (`Ctrl+Shift+T`) : la commande de test du projet (cargo, npm, go, pytest, maven, gradle) dans le terminal ; `Run Test at Caret` (`Ctrl+Shift+F10`) : le test sous le curseur (Rust, Python, Go, JS/TS) | ✅ |
| Panneau **Tests** : les tests du fichier ouvert, un clic lance celui choisi | ✅ |
| **Débogueur** (`crates/lsp/src/dap.rs`, client DAP) : points d'arrêt (clic sur le numéro de ligne ou `F9`), `Ctrl+F5` lance, `F5` continue, `F10` / `F11` / `Shift+F11` pas à pas, `Shift+F5` arrête ; panneau Debug avec pile d'appels, variables et sortie du programme | ✅ |
| Expressions surveillées (watch) : tapées dans le panneau Debug, recalculées à chaque arrêt et quand on change d'appel dans la pile, gardées d'une session à l'autre ; un clic en retire une | ✅ |
| Autres langages que Python : `Edit Debug Configurations` (adaptateur + réglages de lancement, `${file}` et `${folder}`) ; mécanisme fait, aucun autre adaptateur vérifié ici | ✅ |
| `Run Tests and Show Results` (`Ctrl+Alt+T`, ou `Run all` dans le panneau Tests) : la commande de test du projet tourne hors du terminal, et ce qu'elle imprime est lu (cargo, `pytest -v`, `go test -v`, Jest / Vitest verbeux) : tests échoués du projet en tête, coche ou croix sur ceux du fichier ouvert, compte, et toute la sortie à côté. Format vérifié sur une vraie sortie de `cargo test` ; les autres par leurs formats connus | ✅ |
| Tests de tout le projet dans le panneau Tests (`Find all`) : fichier par fichier, un clic va au test, coche ou croix d'après le dernier lancement | ✅ |
| Couverture dans la marge : `Run Tests with Coverage` lance les tests avec leur rapport (cargo llvm-cov, go, pytest-cov, jest), `Show / Hide Coverage` relit un rapport LCOV ou Go déjà là ; ligne atteinte en vert, jamais atteinte en rouge, pourcentage annoncé. Les marques ne suivent pas la frappe | ✅ |
| Relance d'un seul test avec résultat : un clic sur un test du fichier dans le panneau Tests le lance ici, son verdict remplace l'ancien, ceux des autres restent | ✅ |
| Variables dépliables : un clic ouvre une liste ou un objet, ses parties viennent dessous ; points d'arrêt conditionnels : `Breakpoint Condition…` (`Shift+F9`) sur la ligne du curseur, vide pour toujours | ✅ |
| Pendant un arrêt, le survol d'un nom dans le code dit ce qu'il vaut ; un point d'arrêt conditionnel est dessiné creux dans la marge | ✅ (survol non vu à l'écran) |
| Les points d'arrêt suivent leur code quand des lignes sont tapées ou effacées au-dessus (dans l'éditeur ; pas après un reformatage) | ✅ |
| Modifier une valeur pendant un arrêt : clic droit sur une variable ou une de ses parties dans le panneau Debug | ✅ |

Vérifié de bout en bout avec le vrai débogueur Python (debugpy) : arrêt sur un point d'arrêt,
pile, variables et leurs parties, valeur modifiée, expressions surveillées, point d'arrêt
conditionnel,
reprise jusqu'à la fin.
`cargo test -p godcode-lsp --test python -- --ignored` le rejoue ; il lui faut debugpy
(`pip install debugpy`, ou son dossier dans `GODCODE_DEBUGPY`).

### Phase 5 — Git complet et projets (⏳)

Fait : branches, blame, diff avec le dernier commit, journal des commits avec le diff de
chacun, stash et stash pop. merge et rebase d'une branche, conflits réglés au
curseur (garder le mien, le leur, les deux). `Git Graph…` dessine toutes les branches et leurs fusions, un clic montre
le commit. `Conflict: Open Merge View` montre les conflits du fichier
sur trois colonnes (le mien, le résultat, le leur) : `Mine` / `Both` / `Theirs` par conflit, `All mine`,
`All theirs`, `Apply` écrit le résultat. Monorepos : `Run Task…` liste aussi les tâches des projets rangés dans le projet (ses dossiers et ceux de
`packages`, `apps`, `crates`, `services`, `libs`, `modules` qui ont un `package.json`, `Cargo.toml`,
`go.mod` ou `pyproject.toml`), chacune lancée dans son dossier. Variables d'environnement : `Choose .env File…` prend un `.env` du dossier ; ses valeurs vont aux tests
lancés ici, au débogueur Python et au prochain terminal. Fait : `New Project…` crée un projet depuis un
modèle (page web, Node/TypeScript, Python, Rust).

### Phase 6 — outils (⏳)

Fait : client HTTP — fichiers `.http`, `F5` envoie la requête sous le curseur, réponse
(JSON mis en forme) dans le panneau Response ; passe par `curl`. `@nom = valeur` nomme une
valeur du fichier, `{{nom}}` la reprend dans l'adresse, les en-têtes et le corps ; la méthode
`GRAPHQL` poste son corps comme requête, avec ses variables en JSON après une ligne vide.
Environnements : `http-client.env.json` à côté du fichier ou à la racine, `HTTP: Choose
Environment…` choisit le jeu de valeurs, qui l'emporte sur les `@nom` du fichier.
Historique : les 20 dernières requêtes de la session sont listées à gauche de la réponse, un clic
remontre ce que l'une a répondu. Reste : WebSocket.

Fait : **aperçu web en direct** — le dossier est servi sur 127.0.0.1 uniquement, la page se
recharge seule à chaque sauvegarde. `Live Preview` l'affiche dans la fenêtre, à droite de
l'éditeur ; `Live Preview in Browser` l'ouvre dans le navigateur. Le panneau a une barre
d'adresse (n'importe quelle page, un serveur de développement comme `localhost:5173`),
précédent / suivant, recharger, ouvrir dans le navigateur, et **inspecter** : les outils de développement de la
page (éléments, console, réseau).

L'aperçu intégré passe par la vue web du système (WebView2, par la bibliothèque `wry`) :
écrire un moteur web n'est pas à notre portée, c'est la seule dépendance de ce genre.
Windows seulement pour l'instant ; ailleurs la page s'ouvre dans le navigateur. La vue web
est une fenêtre posée sur la nôtre : elle s'efface quand un menu ou une boîte de dialogue
a besoin de la place.

L'adresse suit les liens cliqués dans la page, et l'aperçu suit l'éditeur : une autre page
HTML du dossier servi, mise en avant, prend la place de celle affichée.

Bases de données : ouvrir un fichier SQLite montre ses tables et leurs lignes, page par page (lecture
seule, lecteur maison sans dépendance ; pas de requêtes SQL, pas de tables `WITHOUT ROWID`, le journal
`-wal` n'est pas lu et c'est dit).
Docker : `Docker…` liste conteneurs et images (par la commande `docker`), démarre, arrête, relance un
conteneur, suit ses logs dans le terminal ; rien n'y supprime quoi que ce soit.
Docker compose : quand le dossier a un fichier compose, ses services sont nommés avec `Up` (`up -d`),
`Stop` et `Logs`.
`Shell` ouvre un `sh` dans un conteneur lancé, dans le terminal (sans invite : le terminal d'ici n'est
pas un vrai terminal).
Requêtes : la fenêtre a un champ de question — un `SELECT` sur une table (`WHERE`, `ORDER BY`, `LIMIT`,
`DISTINCT`, `count` / `sum` / `avg` / `min` / `max`, `LIKE`, `IN`, `BETWEEN`, quelques fonctions), vérifié contre le
vrai SQLite sur 250 requêtes ; ni `JOIN` ni `GROUP BY`, et un nombre n'y égale jamais un texte (pas
d'affinité de type). 1 000 lignes gardées au plus.
Reste : PostgreSQL.

### Phase 7 — personnalisation et extensions (⏳)

Fait : 6 thèmes (GodCode Dark/Light, Darcula, Monokai, Solarized Light, High Contrast),
mode Zen (`Ctrl+Alt+Z`). `Editor Font…` choisit le fichier de police (.ttf, .otf) qui dessine le code,
`Reset Editor Font` revient à celle d'origine ; une police à chasse fixe est attendue.
Thèmes de l'utilisateur : `Edit My Themes` ouvre `themes.json` (exemple commenté écrit la première fois) ;
chaque thème y donne son nom, sombre ou clair, les couleurs qu'il change (le reste vient de GodCode
Dark/Light) ; enregistré, il est relu, listé dans `Color Theme…` et appliqué s'il est en cours.
Dispositions : `Save Layout…` garde sous un nom ce qui se montre (panneau de côté, panneau du bas,
minimap, sticky scroll, zen), `Layout…` en remet une ; gardées d'une session à l'autre.
L'interface est en anglais, et le reste : pas d'autres langues.
Reste :
API de plugins (WASM isolé) une fois le cœur stable.

### Phase 8 — IA (⏳, volontairement en dernier)

Fait : assistant contextuel — panneau `ASSISTANT` (`Toggle Assistant`) : une question est envoyée avec la
conversation, le texte du fichier en avant (60 000 caractères au plus) et la sélection. Modèles locaux
par Ollama (`OLLAMA_HOST`, 127.0.0.1:11434 par défaut), ou API Anthropic quand `ANTHROPIC_API_KEY` est
dans l'environnement ; passe par `curl`, la clé n'apparaît jamais sur la ligne de commande. Rien n'est
envoyé tant qu'on n'a pas posé de question. Les réponses sont mises en page comme un fichier Markdown
(code en blocs), `Copy` copie leur texte. Pas de réponse au fil de l'eau.
Complétion : `Assistant: Complete Here` (`Alt+\`) demande au modèle la suite au curseur ; elle s'affiche en
grisé (première ligne et nombre de lignes de plus), `Tab` la prend, `Échap` ou toute frappe la laisse.
À la demande seulement : rien ne part pendant qu'on tape.
Modification avec validation : `Edit file` demande un changement du fichier en avant (10 000 caractères
au plus) ; rien n'est écrit avant `Apply`, `Review` montre l'avant et l'après côte à côte, `Discard` laisse.
Reste : outils MCP dans la conversation, agents qui enchaînent plusieurs étapes.

### Hors périmètre tant que le reste n'est pas solide (❌)

Collaboration temps réel, marketplace, SSO, intégrations cloud (AWS/Azure/GCP), Kubernetes,
développement distant, time-travel debugging. Ce sont des produits à part entière.
