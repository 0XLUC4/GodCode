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

/// The name of the test line `at` of `lines` starts, when it starts one: a Rust `#[test]`
/// function, a Python `test_…` function, a Go `Test…` function, a JavaScript `it("…")`.
fn test_on(lines: &[&str], at: usize, language: &Language) -> Option<String> {
    let content = lines[at].trim_start();
    let name_after = |keyword: &str| -> Option<String> {
        let rest = content.split(keyword).nth(1)?;
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    };
    match language.name {
        "Rust" => {
            let above = lines[..at].iter().rev().take(3);
            let marked = above.into_iter().any(|line| line.contains("#[test]"));
            name_after("fn ").filter(|_| marked)
        }
        "Python" => name_after("def ").filter(|name| name.starts_with("test")),
        "Go" => name_after("func ").filter(|name| name.starts_with("Test")),
        "JavaScript" | "TypeScript" => ["it(", "test("].iter().find_map(|call| {
            let rest = content.strip_prefix(call)?;
            let quote = rest.chars().next().filter(|ch| "\"'`".contains(*ch))?;
            Some(rest[1..].split(quote).next()?.to_owned())
        }),
        _ => None,
    }
}

/// The name of the test written at or above line `line` (counted from 0).
pub fn test_at(text: &str, language: &Language, line: usize) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let last = line.min(lines.len().checked_sub(1)?);
    (0..=last)
        .rev()
        .find_map(|at| test_on(&lines, at, language))
}

/// Every test of `text`, with the line (from 0) it starts on.
pub fn tests(text: &str, language: &Language) -> Vec<(usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let found = (0..lines.len()).filter_map(|at| Some((at, test_on(&lines, at, language)?)));
    found.collect()
}

/// The tests a run says it ran, with whether each passed: read from what `cargo test`,
/// `pytest -v`, `go test -v` and the verbose reporters of Jest and Vitest print. A test is
/// named alone, without its module or its file.
pub fn results(output: &str) -> Vec<(String, bool)> {
    let alone = |path: &str| path.rsplit("::").next().unwrap_or(path).to_owned();
    let read = |line: &str| {
        let line = line.trim();
        // cargo: `test tests::adds ... ok`
        if let Some((name, verdict)) = line
            .strip_prefix("test ")
            .and_then(|rest| rest.rsplit_once(" ... "))
        {
            return match verdict.trim() {
                "ok" => Some((alone(name), true)),
                "FAILED" => Some((alone(name), false)),
                _ => None,
            };
        }
        // go: `--- PASS: TestAdds (0.00s)`
        for (mark, passed) in [("--- PASS: ", true), ("--- FAIL: ", false)] {
            if let Some(rest) = line.strip_prefix(mark) {
                return Some((rest.split_whitespace().next()?.to_owned(), passed));
            }
        }
        // pytest: `tests/test_sum.py::test_adds PASSED   [ 50%]`
        for (mark, passed) in [(" PASSED", true), (" FAILED", false)] {
            let named = line.split_once(mark).map(|(name, _)| name);
            if let Some(name) = named.filter(|name| name.contains("::") && !name.contains(' ')) {
                return Some((alone(name), passed));
            }
        }
        // Jest and Vitest: `✓ adds (2 ms)`
        for (mark, passed) in [("✓ ", true), ("√ ", true), ("✕ ", false), ("× ", false)] {
            if let Some(rest) = line.strip_prefix(mark) {
                let name = rest.rsplit_once(" (").map_or(rest, |(name, _)| name);
                return Some((name.trim().to_owned(), passed));
            }
        }
        None
    };
    output.lines().filter_map(read).collect()
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

    #[test]
    fn a_test_run_is_read_for_what_passed() {
        let cargo = "running 3 tests\ntest edit::tests::adds ... ok\ntest tests::breaks ... FAILED\n\
                     test slow ... ignored\n\ntest result: FAILED. 1 passed; 1 failed; 1 ignored";
        let named = |name: &str, passed| (name.to_owned(), passed);
        assert_eq!(
            results(cargo),
            [named("adds", true), named("breaks", false)]
        );
        let pytest = "tests/test_sum.py::test_adds PASSED      [ 50%]\n\
                      tests/test_sum.py::TestMore::test_breaks FAILED [100%]\n\
                      FAILED tests/test_sum.py::TestMore::test_breaks - assert 1 == 2";
        assert_eq!(
            results(pytest),
            [named("test_adds", true), named("test_breaks", false)]
        );
        let go = "=== RUN   TestAdds\n--- PASS: TestAdds (0.00s)\n    --- FAIL: TestBreaks (0.01s)\nFAIL";
        assert_eq!(
            results(go),
            [named("TestAdds", true), named("TestBreaks", false)]
        );
        let jest = " ✓ adds two numbers (2 ms)\n ✕ breaks\n × breaks too 3ms";
        assert_eq!(
            results(jest),
            [
                named("adds two numbers", true),
                named("breaks", false),
                named("breaks too 3ms", false)
            ]
        );
        assert!(results("Compiling a v0.1.0\nnothing here").is_empty());
    }
    use std::path::Path;

    #[test]
    fn the_test_around_a_line_is_named() {
        let named = |text: &str, file: &str, line: usize| {
            test_at(text, Language::of(Path::new(file)), line)
        };
        let rust = "fn helper() {}\n\n#[test]\nfn adds_up() {\n    assert!(true);\n}";
        assert_eq!(named(rust, "a.rs", 4).as_deref(), Some("adds_up"));
        assert_eq!(named(rust, "a.rs", 0), None);
        let python = "def helper():\n    pass\n\ndef test_sum():\n    assert 1";
        assert_eq!(named(python, "a.py", 4).as_deref(), Some("test_sum"));
        let script =
            "describe('x', () => {\n  it(\"adds two\", () => {\n    expect(1);\n  });\n});";
        assert_eq!(named(script, "a.ts", 2).as_deref(), Some("adds two"));
        assert_eq!(
            named("func TestSum(t *testing.T) {\n}", "a_test.go", 1).as_deref(),
            Some("TestSum")
        );
        assert_eq!(named("", "a.rs", 0), None);
        let all = tests(python, Language::of(Path::new("a.py")));
        assert_eq!(all, [(3, "test_sum".to_owned())]);
    }

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
