//! The words of the interface in the language of the user.

use std::sync::atomic::{AtomicU8, Ordering};

/// A language the interface can be shown in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tongue {
    #[default]
    English,
    French,
}

impl Tongue {
    pub const ALL: [Tongue; 2] = [Tongue::English, Tongue::French];

    /// The name of the tongue, in that tongue.
    pub fn name(self) -> &'static str {
        match self {
            Tongue::English => "English",
            Tongue::French => "Français",
        }
    }

    /// What is kept between sessions.
    pub fn code(self) -> &'static str {
        match self {
            Tongue::English => "en",
            Tongue::French => "fr",
        }
    }

    /// The tongue kept as `code`; None for a code no tongue has.
    pub fn of_code(code: &str) -> Option<Tongue> {
        Tongue::ALL.into_iter().find(|tongue| tongue.code() == code)
    }
}

/// The tongue in use, as its place in [`Tongue::ALL`].
static IN_USE: AtomicU8 = AtomicU8::new(0);

/// Shows the interface in `tongue` from now on, in the whole process.
pub fn set(tongue: Tongue) {
    IN_USE.store(tongue as u8, Ordering::Relaxed);
}

/// The tongue the interface is shown in.
pub fn current() -> Tongue {
    match IN_USE.load(Ordering::Relaxed) {
        1 => Tongue::French,
        _ => Tongue::English,
    }
}

/// `text` in the tongue in use; `text` itself when it is English or has no translation.
pub fn tr(text: &'static str) -> &'static str {
    in_tongue(current(), text)
}

/// `text` in `tongue`; `text` itself in English or when it has no translation.
pub fn in_tongue(tongue: Tongue, text: &'static str) -> &'static str {
    match tongue {
        Tongue::English => text,
        Tongue::French => FRENCH
            .binary_search_by_key(&text, |&(english, _)| english)
            .map_or(text, |place| FRENCH[place].1),
    }
}

/// The French of each English text of the interface, sorted by the English for the binary
/// search. Sentence case, a space before `:`, and the nouns developers keep in English
/// (Git, commit, stash, merge, rebase, blame…) left as they are.
const FRENCH: &[(&str, &str)] = &[
    ("ASSISTANT", "ASSISTANT IA"),
    ("Add Caret Above", "Ajouter un curseur au-dessus"),
    ("Add Caret Below", "Ajouter un curseur en dessous"),
    ("All mine", "Ma version partout"),
    ("All theirs", "Leur version partout"),
    ("Apply", "Appliquer"),
    ("Back", "Revenir en arrière"),
    ("Both", "Les deux"),
    ("Breakpoint Condition…", "Condition du point d'arrêt…"),
    ("CALL STACK", "PILE D'APPELS"),
    ("Call Hierarchy: Callers", "Hiérarchie d'appels : appelants"),
    (
        "Call Hierarchy: Calls Made",
        "Hiérarchie d'appels : appels effectués",
    ),
    ("Cancel", "Annuler"),
    ("Check for Problems", "Rechercher des problèmes"),
    ("Checking…", "Vérification…"),
    ("Choose .env File…", "Choisir le fichier .env…"),
    ("Clear Terminal", "Effacer le terminal"),
    ("Clear terminal", "Effacer le terminal"),
    ("Close", "Fermer"),
    ("Close All", "Tout fermer"),
    ("Close All Editors", "Fermer tous les éditeurs"),
    ("Close Editor", "Fermer l'éditeur"),
    ("Close Folder", "Fermer le dossier"),
    ("Close Other Editors", "Fermer les autres éditeurs"),
    ("Close Others", "Fermer les autres"),
    ("Close panel", "Fermer le panneau"),
    ("Close the preview", "Fermer l'aperçu"),
    ("Close to the Right", "Fermer à droite"),
    ("Code", "Code"),
    ("Color Theme…", "Thème de couleur…"),
    ("Color theme", "Thème de couleur"),
    ("Command Palette…", "Palette de commandes…"),
    ("Commit all", "Tout commiter"),
    ("Commit message", "Message de commit"),
    (
        "Compare with Last Commit",
        "Comparer avec le dernier commit",
    ),
    ("Conflict: Keep Both", "Conflit : garder les deux"),
    ("Conflict: Keep Mine", "Conflit : garder ma version"),
    ("Conflict: Keep Theirs", "Conflit : garder leur version"),
    (
        "Conflict: Open Merge View",
        "Conflit : ouvrir la vue de merge",
    ),
    ("Copy", "Copier"),
    ("Copy Path", "Copier le chemin"),
    ("Create", "Créer"),
    ("Cut", "Couper"),
    ("DEBUG", "DÉBOGAGE"),
    ("Dark mode", "Mode sombre"),
    ("Debug", "Débogage"),
    ("Debug File", "Déboguer le fichier"),
    ("Delete", "Supprimer"),
    ("Delete Line", "Supprimer la ligne"),
    ("Docker…", "Docker…"),
    ("Duplicate Line", "Dupliquer la ligne"),
    ("EXPLORER", "EXPLORATEUR"),
    ("Edit", "Édition"),
    (
        "Edit Debug Configurations",
        "Modifier les configurations de débogage",
    ),
    ("Edit Keyboard Shortcuts", "Modifier les raccourcis clavier"),
    ("Edit My Themes", "Modifier mes thèmes"),
    (
        "Edit Run Configurations",
        "Modifier les configurations d'exécution",
    ),
    ("Edit Snippets", "Modifier les extraits de code"),
    ("Editor Font…", "Police de l'éditeur…"),
    ("Environment file", "Fichier d'environnement"),
    ("Explorer", "Explorateur"),
    ("Extend Selection", "Étendre la sélection"),
    ("File", "Fichier"),
    ("Find", "Rechercher"),
    ("Find Next", "Occurrence suivante"),
    ("Find Previous", "Occurrence précédente"),
    ("Find Usages", "Rechercher les utilisations"),
    ("Find all", "Tout rechercher"),
    ("Find and Replace", "Rechercher et remplacer"),
    ("Find in Files", "Rechercher dans les fichiers"),
    ("Fold / Unfold Block", "Plier / déplier le bloc"),
    ("Forward", "Avancer"),
    ("Git", "Git"),
    ("Git Branches…", "Git : branches…"),
    ("Git Graph…", "Git : graphe…"),
    ("Git Log…", "Git : log…"),
    ("Git Merge Branch…", "Git : merge d'une branche…"),
    ("Git Rebase onto…", "Git : rebase sur…"),
    ("Git Stash", "Git : stash"),
    ("Git Stash Pop", "Git : stash pop"),
    ("Git graph", "Graphe Git"),
    ("Git log", "Log Git"),
    ("Go to Declaration", "Atteindre la déclaration"),
    ("Go to Implementation", "Atteindre l'implémentation"),
    ("Go to Line…", "Atteindre la ligne…"),
    ("Go to Next Problem", "Atteindre le problème suivant"),
    ("Go to Symbol…", "Atteindre le symbole…"),
    ("HTTP environment", "Environnement HTTP"),
    (
        "HTTP: Choose Environment…",
        "HTTP : choisir l'environnement…",
    ),
    ("Help", "Aide"),
    ("Indent Lines", "Indenter les lignes"),
    ("Initialize repository", "Initialiser le dépôt"),
    ("Interface Language", "Langue de l'interface"),
    ("Join Lines", "Joindre les lignes"),
    ("Keyboard Shortcuts", "Raccourcis clavier"),
    ("Keyboard shortcuts", "Raccourcis clavier"),
    ("Language Server Status", "État du serveur de langage"),
    ("Language server", "Serveur de langage"),
    ("Layout", "Disposition"),
    ("Layout…", "Disposition…"),
    ("Light mode", "Mode clair"),
    ("Live Preview", "Aperçu en direct"),
    (
        "Live Preview in Browser",
        "Aperçu en direct dans le navigateur",
    ),
    ("Local History…", "Historique local…"),
    ("Local history", "Historique local"),
    ("Match case", "Respecter la casse"),
    ("Mine", "Ma version"),
    ("Move Line Down", "Déplacer la ligne vers le bas"),
    ("Move Line Up", "Déplacer la ligne vers le haut"),
    ("Move to recycle bin", "Déplacer vers la corbeille"),
    ("Name", "Nom"),
    ("Navigate", "Navigation"),
    ("New File", "Nouveau fichier"),
    ("New Project…", "Nouveau projet…"),
    ("New file", "Nouveau fichier"),
    ("New folder", "Nouveau dossier"),
    ("New project", "Nouveau projet"),
    ("New value", "Nouvelle valeur"),
    ("Next", "Suivant"),
    ("Next Editor", "Éditeur suivant"),
    ("No .env file", "Aucun fichier .env"),
    ("No environment", "Aucun environnement"),
    ("No folder opened", "Aucun dossier ouvert"),
    ("No results", "Aucun résultat"),
    ("OUTPUT", "SORTIE"),
    ("Open File…", "Ouvrir un fichier…"),
    ("Open Folder…", "Ouvrir un dossier…"),
    ("Open file…", "Ouvrir un fichier…"),
    ("Open folder…", "Ouvrir un dossier…"),
    ("Open in the browser", "Ouvrir dans le navigateur"),
    ("Outdent Lines", "Désindenter les lignes"),
    ("PROBLEMS", "PROBLÈMES"),
    ("PROJECT", "PROJET"),
    ("Paste", "Coller"),
    ("Pin or Unpin", "Épingler ou désépingler"),
    ("Previous", "Précédent"),
    ("Previous Editor", "Éditeur précédent"),
    ("Problems", "Problèmes"),
    ("Quick Documentation", "Documentation rapide"),
    ("Quick fixes", "Corrections rapides"),
    ("Quit", "Quitter"),
    ("RECENT", "RÉCENTS"),
    ("RESPONSE", "RÉPONSE"),
    ("Recent Files", "Fichiers récents"),
    ("Redo", "Rétablir"),
    ("Reformat Code", "Reformater le code"),
    ("Refresh", "Actualiser"),
    ("Regular expression", "Expression régulière"),
    ("Reload", "Recharger"),
    ("Rename", "Renommer"),
    (
        "Rename All Occurrences…",
        "Renommer toutes les occurrences…",
    ),
    ("Rename all occurrences", "Renommer toutes les occurrences"),
    ("Replace", "Remplacer"),
    ("Replace all", "Tout remplacer"),
    ("Reset Editor Font", "Réinitialiser la police de l'éditeur"),
    ("Reset Zoom", "Réinitialiser le zoom"),
    ("Response", "Réponse"),
    ("Restart", "Redémarrer"),
    ("Restart Terminal", "Redémarrer le terminal"),
    ("Restart terminal", "Redémarrer le terminal"),
    ("Run", "Exécuter"),
    ("Run File", "Exécuter le fichier"),
    ("Run Last Task", "Exécuter la dernière tâche"),
    ("Run Task…", "Exécuter une tâche…"),
    ("Run Test at Caret", "Exécuter le test sous le curseur"),
    ("Run Tests", "Exécuter les tests"),
    (
        "Run Tests and Show Results",
        "Exécuter les tests et afficher les résultats",
    ),
    (
        "Run Tests with Coverage",
        "Exécuter les tests avec la couverture",
    ),
    ("Run all", "Tout exécuter"),
    ("Run task", "Exécuter une tâche"),
    ("Running…", "Exécution…"),
    ("SEARCH", "RECHERCHE"),
    ("SOURCE CONTROL", "CONTRÔLE DE CODE SOURCE"),
    ("Save", "Enregistrer"),
    ("Save All", "Tout enregistrer"),
    ("Save As…", "Enregistrer sous…"),
    ("Save Layout…", "Enregistrer la disposition…"),
    ("Save this layout as", "Enregistrer cette disposition sous"),
    ("Search", "Recherche"),
    ("Search Everywhere…", "Rechercher partout…"),
    ("Search in files", "Rechercher dans les fichiers"),
    ("Select All", "Tout sélectionner"),
    (
        "Select File in Explorer",
        "Sélectionner le fichier dans l'explorateur",
    ),
    ("Select in Explorer", "Sélectionner dans l'explorateur"),
    ("Set", "Définir"),
    ("Show / Hide Coverage", "Afficher / masquer la couverture"),
    ("Show Explorer", "Afficher l'explorateur"),
    ("Show Quick Fixes", "Afficher les corrections rapides"),
    ("Show Source Control", "Afficher le contrôle de code source"),
    ("Show Structure", "Afficher la structure"),
    ("Shrink Selection", "Réduire la sélection"),
    ("Source control", "Contrôle de code source"),
    ("Split Editor Down", "Fractionner l'éditeur en bas"),
    ("Split Editor Right", "Fractionner l'éditeur à droite"),
    (
        "Split Editor: Same File",
        "Fractionner l'éditeur : même fichier",
    ),
    ("Start", "Démarrer"),
    ("Step Into", "Pas à pas détaillé"),
    ("Step Out", "Pas à pas sortant"),
    ("Step Over", "Pas à pas principal"),
    ("Stop", "Arrêter"),
    ("Stop Debugging", "Arrêter le débogage"),
    ("Suggest Completions", "Suggérer des complétions"),
    (
        "Switch Keys: Default / Vim / Emacs",
        "Changer de touches : par défaut / Vim / Emacs",
    ),
    (
        "Switch or create a branch",
        "Changer de branche ou en créer une",
    ),
    ("Theirs", "Leur version"),
    ("Toggle Assistant", "Basculer l'assistant"),
    ("Toggle Auto Save", "Basculer l'enregistrement automatique"),
    ("Toggle Breakpoint", "Basculer le point d'arrêt"),
    ("Toggle Case", "Basculer la casse"),
    ("Toggle Debug Panel", "Basculer le panneau de débogage"),
    (
        "Toggle Format on Save",
        "Basculer la mise en forme à l'enregistrement",
    ),
    ("Toggle Git Blame", "Basculer le blame Git"),
    ("Toggle HTTP Response", "Basculer la réponse HTTP"),
    ("Toggle Inlay Hints", "Basculer les indications en ligne"),
    (
        "Toggle Light / Dark Theme",
        "Basculer le thème clair / sombre",
    ),
    ("Toggle Line Comment", "Basculer le commentaire de ligne"),
    ("Toggle Markdown Preview", "Basculer l'aperçu Markdown"),
    ("Toggle Minimap", "Basculer la minimap"),
    ("Toggle Problems", "Basculer les problèmes"),
    ("Toggle Side Bar", "Basculer la barre latérale"),
    (
        "Toggle Soft Wrap",
        "Basculer le retour automatique à la ligne",
    ),
    ("Toggle Sticky Scroll", "Basculer le défilement épinglé"),
    ("Toggle TODO", "Basculer les TODO"),
    ("Toggle Terminal", "Basculer le terminal"),
    ("Toggle Tests", "Basculer les tests"),
    ("Toggle Zen Mode", "Basculer le mode zen"),
    ("Undo", "Annuler"),
    ("Unfold All", "Tout déplier"),
    ("Unsaved changes", "Modifications non enregistrées"),
    ("View", "Affichage"),
    ("WATCH", "ESPIONS"),
    ("Welcome Page", "Page d'accueil"),
    ("Whole word", "Mot entier"),
    ("Zoom In", "Zoom avant"),
    ("Zoom Out", "Zoom arrière"),
    ("Zoom in", "Zoom avant"),
    ("Zoom out", "Zoom arrière"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts that read the same in French: names of products and words French shares.
    const SAME_IN_FRENCH: [&str; 3] = ["Code", "Docker…", "Git"];

    /// The labels of the commands, read in the source of the application: the first
    /// string of each `spec(…)` of its table, whose two first arguments are no strings.
    fn command_labels() -> Vec<String> {
        const START: &str = "const COMMANDS: &[Spec] = &[";
        let source = include_str!("app.rs");
        let start = source.find(START).expect("the table of commands") + START.len();
        let table = &source[start..];
        let table = &table[..table.find("\n];").expect("the end of the table")];
        let labels: Vec<String> = table
            .split("spec(")
            .skip(1)
            .map(|call| {
                let opened = &call[call.find('"').expect("a label") + 1..];
                let mut label = String::new();
                let mut letters = opened.chars();
                loop {
                    match letters.next().expect("the end of the label") {
                        '"' => break label,
                        '\\' => match letters.next() {
                            Some(escaped @ ('"' | '\\')) => label.push(escaped),
                            other => panic!("escape not read by this test: {other:?}"),
                        },
                        letter => label.push(letter),
                    }
                }
            })
            .collect();
        assert_eq!(labels.len(), table.matches("Cmd::").count());
        labels
    }

    #[test]
    fn every_command_has_its_french() {
        let labels = command_labels();
        assert!(labels.len() > 120, "only {} labels read", labels.len());
        let missing: Vec<&String> = labels
            .iter()
            .filter(|label| !FRENCH.iter().any(|(english, _)| english == label))
            .collect();
        assert!(missing.is_empty(), "no French for {missing:?}");
    }

    #[test]
    fn the_table_is_sorted_and_says_each_text_once() {
        for pair in FRENCH.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "{:?} before {:?}",
                pair[0].0,
                pair[1].0
            );
        }
    }

    #[test]
    fn the_french_differs_from_the_english_but_for_the_words_both_share() {
        for (english, french) in FRENCH {
            let same = english == french;
            assert_eq!(same, SAME_IN_FRENCH.contains(english), "{english:?}");
        }
    }

    #[test]
    fn the_french_ends_with_an_ellipsis_where_the_english_does_and_only_there() {
        for (english, french) in FRENCH {
            assert_eq!(english.ends_with('…'), french.ends_with('…'), "{english:?}");
        }
    }

    #[test]
    fn the_french_is_trimmed_and_puts_a_space_before_a_colon() {
        for (english, french) in FRENCH {
            assert_eq!(french.trim(), *french, "{english:?}");
            assert!(!french.is_empty(), "{english:?}");
            let colons = french.matches(':').count();
            assert!(colons >= english.matches(':').count(), "{english:?}");
            assert_eq!(colons, french.matches(" : ").count(), "{english:?}");
        }
    }

    #[test]
    fn a_text_is_looked_up_in_the_tongue_asked() {
        assert_eq!(in_tongue(Tongue::French, "Save As…"), "Enregistrer sous…");
        assert_eq!(in_tongue(Tongue::French, "Undo"), "Annuler");
        assert_eq!(in_tongue(Tongue::French, "File"), "Fichier");
        assert_eq!(in_tongue(Tongue::English, "Save As…"), "Save As…");
        for (english, french) in FRENCH {
            assert_eq!(in_tongue(Tongue::French, english), *french);
            assert_eq!(in_tongue(Tongue::English, english), *english);
        }
    }

    #[test]
    fn a_text_without_french_is_given_back_as_it_is() {
        assert_eq!(in_tongue(Tongue::French, "No such text"), "No such text");
        assert_eq!(in_tongue(Tongue::French, ""), "");
    }

    /// The only test that changes the tongue of the process: it leaves English behind it.
    #[test]
    fn the_tongue_set_is_the_one_in_use_and_the_one_of_tr() {
        assert_eq!(current(), Tongue::English);
        assert_eq!(tr("Save"), "Save");
        assert_eq!(tr("No such text"), "No such text");
        for tongue in Tongue::ALL {
            set(tongue);
            assert_eq!(current(), tongue);
            assert_eq!(tr("Save"), in_tongue(tongue, "Save"));
        }
        set(Tongue::English);
        assert_eq!(current(), Tongue::default());
        assert_eq!(tr("Save"), "Save");
    }

    #[test]
    fn a_tongue_is_found_again_by_its_code() {
        for tongue in Tongue::ALL {
            assert_eq!(Tongue::of_code(tongue.code()), Some(tongue));
        }
        assert_eq!(Tongue::of_code("de"), None);
        assert_eq!(Tongue::of_code(""), None);
        assert_eq!(Tongue::French.name(), "Français");
        assert_eq!(Tongue::default().name(), "English");
    }
}
