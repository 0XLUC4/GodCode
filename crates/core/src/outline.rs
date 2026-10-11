//! The outline of a file: its functions, types and headings, found line by line.

use crate::syntax::Language;

/// Words that may stand before a definition.
const MODIFIERS: [&str; 17] = [
    "pub",
    "pub(crate)",
    "pub(super)",
    "async",
    "export",
    "default",
    "declare",
    "static",
    "public",
    "private",
    "protected",
    "abstract",
    "final",
    "unsafe",
    "const",
    "extern",
    "inline",
];

/// Words that start a definition, in the languages GodCode colours.
const DEFINERS: [&str; 14] = [
    "fn",
    "struct",
    "enum",
    "trait",
    "impl",
    "mod",
    "class",
    "def",
    "function",
    "interface",
    "func",
    "type",
    "namespace",
    "module",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// The word that defines it: `fn`, `class`, `heading`…
    pub kind: &'static str,
    /// Line counted from 0.
    pub line: usize,
}

fn definition(line: &str) -> Option<(&'static str, String)> {
    let mut words = line.split_whitespace().peekable();
    while words.next_if(|word| MODIFIERS.contains(word)).is_some() {}
    let first = words.next()?;
    let kind = *DEFINERS.iter().find(|definer| **definer == first)?;
    let rest = words.collect::<Vec<_>>().join(" ");
    let name: String = if kind == "impl" {
        // `impl Trait for Type` reads best whole.
        rest.split('{').next().unwrap_or("").trim().to_owned()
    } else {
        rest.chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect()
    };
    (!name.is_empty()).then_some((kind, name))
}

/// The elements of `content` that carry an `id`, as `#id`.
fn anchors(content: &str) -> impl Iterator<Item = String> + '_ {
    content.split(" id=").skip(1).filter_map(|rest| {
        let quote = rest.chars().next().filter(|ch| *ch == '"' || *ch == '\'')?;
        let id = rest[1..].split(quote).next()?;
        (!id.is_empty()).then(|| format!("#{id}"))
    })
}

/// The definitions of `text`, in order.
pub fn symbols(text: &str, language: &Language) -> Vec<Symbol> {
    let markdown = language.name == "Markdown";
    if language.is_markup() {
        return text
            .lines()
            .enumerate()
            .flat_map(|(line, content)| {
                anchors(content).map(move |name| Symbol {
                    name,
                    kind: "id",
                    line,
                })
            })
            .collect();
    }
    let css = language.name == "CSS";
    text.lines()
        .enumerate()
        .filter_map(|(line, content)| {
            if css {
                let selector = content.trim().strip_suffix('{')?.trim();
                return (!selector.is_empty()).then(|| Symbol {
                    name: selector.to_owned(),
                    kind: "rule",
                    line,
                });
            }
            let (kind, name) = if markdown {
                let title = content.strip_prefix('#')?.trim_start_matches('#').trim();
                ("heading", title.to_owned())
            } else {
                definition(content)?
            };
            Some(Symbol { name, kind, line })
        })
        .collect()
}

/// Words that introduce a name without being in the outline: variables and constants.
const BINDERS: [&str; 6] = ["let mut ", "const ", "let ", "var ", "static ", "val "];

/// The line (counted from 0) where `word` is defined in `text`: a definition of the
/// outline, else a variable.
pub fn declaration(text: &str, language: &Language, word: &str) -> Option<usize> {
    let defined = symbols(text, language)
        .into_iter()
        .find(|symbol| symbol.name == word)
        .map(|symbol| symbol.line);
    let is_word = |ch: char| ch.is_alphanumeric() || ch == '_';
    defined.or_else(|| {
        text.lines().position(|line| {
            BINDERS.iter().any(|binder| {
                line.match_indices(binder).any(|(at, _)| {
                    line[at + binder.len()..]
                        .trim_start()
                        .strip_prefix(word)
                        .is_some_and(|after| !after.starts_with(is_word))
                })
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_word_leads_to_its_declaration() {
        let script = "import x from 'y';\nconst total = 1;\nfunction add(a) {\n  let sum = a;\n}";
        let language = Language::of(Path::new("a.js"));
        assert_eq!(declaration(script, language, "add"), Some(2));
        assert_eq!(declaration(script, language, "total"), Some(1));
        assert_eq!(declaration(script, language, "sum"), Some(3));
        assert_eq!(declaration(script, language, "tot"), None);
        assert_eq!(declaration(script, language, "x"), None);
    }

    fn outline(text: &str, file: &str) -> Vec<(String, &'static str, usize)> {
        symbols(text, Language::of(Path::new(file)))
            .into_iter()
            .map(|symbol| (symbol.name, symbol.kind, symbol.line))
            .collect()
    }

    #[test]
    fn definitions_are_listed_with_their_line() {
        let rust = "pub struct App;\n\nimpl Default for App {\n    pub(crate) async fn run(&self) {}\n}\nconst X: u8 = 1;";
        assert_eq!(
            outline(rust, "a.rs"),
            [
                ("App".to_owned(), "struct", 0),
                ("Default for App".to_owned(), "impl", 2),
                ("run".to_owned(), "fn", 3),
            ]
        );
        let python = "class Shop(Base):\n    def open(self):\n        type(self)\n";
        assert_eq!(
            outline(python, "a.py"),
            [
                ("Shop".to_owned(), "class", 0),
                ("open".to_owned(), "def", 1)
            ]
        );
        assert_eq!(
            outline("export default function start() {}", "a.js"),
            [("start".to_owned(), "function", 0)]
        );
    }

    #[test]
    fn markup_lists_its_ids_and_styles_their_rules() {
        assert_eq!(
            outline("<main id=\"top\">\n<a id='x' href=\"#\">", "a.html"),
            [("#top".to_owned(), "id", 0), ("#x".to_owned(), "id", 1)]
        );
        assert_eq!(
            outline(".a, b > c {\n  color: red;\n}\n@media print {", "a.css"),
            [
                (".a, b > c".to_owned(), "rule", 0),
                ("@media print".to_owned(), "rule", 3)
            ]
        );
    }

    #[test]
    fn markdown_lists_its_headings() {
        assert_eq!(
            outline("# Title\ntext # not\n## Part", "a.md"),
            [
                ("Title".to_owned(), "heading", 0),
                ("Part".to_owned(), "heading", 2)
            ]
        );
    }
}
