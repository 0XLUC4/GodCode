//! Completion of the word being typed: snippets, keywords and words of the file.

use std::collections::HashSet;
use std::ops::Range;

use crate::byte_index;
use crate::syntax::Language;

const MAX_COMPLETIONS: usize = 30;
/// Shorter words are quicker typed than picked.
const MIN_WORD_CHARS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Snippet,
    Keyword,
    Word,
    /// A name the language server knows: a method, a field, a type.
    Symbol,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    pub label: String,
    /// What is inserted. In snippets a tab is one indent level and `$0` where the caret ends.
    pub body: String,
    pub kind: Kind,
}

type Snippets = &'static [(&'static str, &'static str)];

const C_LIKE: Snippets = &[
    ("for", "for (int i = 0; i < $0; i++) {\n\t\n}"),
    ("if", "if ($0) {\n\t\n}"),
    ("ifelse", "if ($0) {\n\t\n} else {\n\t\n}"),
    ("while", "while ($0) {\n\t\n}"),
    ("dowhile", "do {\n\t\n} while ($0);"),
    (
        "switch",
        "switch ($0) {\n\tcase :\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}",
    ),
];

/// What TypeScript has that JavaScript has not.
const TYPED: Snippets = &[
    ("interface", "interface $0 {\n\t\n}"),
    ("type", "type $0 = ;"),
    ("enum", "enum $0 {\n\t\n}"),
];

const SCRIPT: Snippets = &[
    ("function", "function $0() {\n\t\n}"),
    ("for", "for (let i = 0; i < $0; i++) {\n\t\n}"),
    ("forof", "for (const item of $0) {\n\t\n}"),
    ("if", "if ($0) {\n\t\n}"),
    ("class", "class $0 {\n\tconstructor() {\n\t\t\n\t}\n}"),
    ("log", "console.log($0);"),
    ("try", "try {\n\t$0\n} catch (error) {\n\t\n}"),
    ("main", "function main() {\n\t$0\n}\n\nmain();"),
    ("arrow", "const $0 = () => {\n\t\n};"),
    ("async", "async function $0() {\n\t\n}"),
    ("ifelse", "if ($0) {\n\t\n} else {\n\t\n}"),
    ("while", "while ($0) {\n\t\n}"),
    (
        "switch",
        "switch ($0) {\n\tcase :\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}",
    ),
    ("forin", "for (const key in $0) {\n\t\n}"),
    ("foreach", "$0.forEach((item) => {\n\t\n});"),
    ("import", "import { $0 } from \"\";"),
    ("export", "export function $0() {\n\t\n}"),
    ("promise", "new Promise((resolve, reject) => {\n\t$0\n});"),
    (
        "fetch",
        "const response = await fetch($0);\nconst data = await response.json();",
    ),
    ("timeout", "setTimeout(() => {\n\t$0\n}, 1000);"),
];

/// Snippets by language name.
const SNIPPETS: &[(&str, Snippets)] = &[
    (
        "Rust",
        &[
            ("fn", "fn $0() {\n\t\n}"),
            ("for", "for $0 in  {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("impl", "impl $0 {\n\t\n}"),
            ("struct", "struct $0 {\n\t\n}"),
            ("enum", "enum $0 {\n\t\n}"),
            ("match", "match $0 {\n\t_ => {}\n}"),
            ("println", "println!(\"$0\");"),
            ("test", "#[test]\nfn $0() {\n\t\n}"),
            ("main", "fn main() {\n\t$0\n}"),
            ("pubfn", "pub fn $0() {\n\t\n}"),
            ("while", "while $0 {\n\t\n}"),
            ("loop", "loop {\n\t$0\n}"),
            ("iflet", "if let Some($0) =  {\n\t\n}"),
            ("ifelse", "if $0 {\n\t\n} else {\n\t\n}"),
            ("trait", "trait $0 {\n\t\n}"),
            ("mod", "mod $0 {\n\t\n}"),
            (
                "tests",
                "#[cfg(test)]\nmod tests {\n\tuse super::*;\n\n\t#[test]\n\tfn $0() {\n\t\t\n\t}\n}",
            ),
            (
                "derive",
                "#[derive(Debug, Clone, PartialEq)]\nstruct $0 {\n\t\n}",
            ),
            ("vec", "vec![$0]"),
            ("dbg", "dbg!($0);"),
        ],
    ),
    (
        "Python",
        &[
            ("def", "def $0():\n\tpass"),
            ("class", "class $0:\n\tdef __init__(self):\n\t\tpass"),
            ("for", "for $0 in :\n\tpass"),
            ("if", "if $0:\n\tpass"),
            ("ifmain", "if __name__ == \"__main__\":\n\t$0"),
            ("print", "print($0)"),
            ("try", "try:\n\t$0\nexcept Exception as error:\n\tpass"),
            (
                "main",
                "def main():\n\t$0\n\n\nif __name__ == \"__main__\":\n\tmain()",
            ),
            ("while", "while $0:\n\tpass"),
            ("ifelse", "if $0:\n\tpass\nelse:\n\tpass"),
            ("with", "with $0 as :\n\tpass"),
            ("async", "async def $0():\n\tpass"),
            ("init", "def __init__(self$0):\n\tpass"),
            ("dataclass", "@dataclass\nclass $0:\n\tpass"),
            ("lambda", "lambda $0: "),
        ],
    ),
    ("JavaScript", SCRIPT),
    ("TypeScript", SCRIPT),
    ("TypeScript", TYPED),
    (
        "Go",
        &[
            ("func", "func $0() {\n\t\n}"),
            ("for", "for $0 {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("iferr", "if err != nil {\n\treturn $0\n}"),
            (
                "main",
                "package main\n\nimport \"fmt\"\n\nfunc main() {\n\t$0\n}",
            ),
            ("struct", "type $0 struct {\n\t\n}"),
            ("interface", "type $0 interface {\n\t\n}"),
            ("method", "func (r *Receiver) $0() {\n\t\n}"),
            ("forrange", "for i, item := range $0 {\n\t\n}"),
            ("switch", "switch $0 {\ncase :\n\t\ndefault:\n\t\n}"),
            ("println", "fmt.Println($0)"),
            ("test", "func Test$0(t *testing.T) {\n\t\n}"),
        ],
    ),
    (
        "HTML",
        &[(
            "html5",
            "<!doctype html>\n<html lang=\"en\">\n<head>\n\t<meta charset=\"utf-8\">\n\t<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\t<title>$0</title>\n</head>\n<body>\n\t\n</body>\n</html>",
        )],
    ),
    ("C", C_LIKE),
    (
        "C",
        &[
            (
                "main",
                "int main(int argc, char *argv[]) {\n\t$0\n\treturn 0;\n}",
            ),
            ("include", "#include <$0>"),
            ("struct", "struct $0 {\n\t\n};"),
            ("printf", "printf(\"$0\\n\");"),
        ],
    ),
    ("C++", C_LIKE),
    (
        "C++",
        &[
            ("main", "int main() {\n\t$0\n\treturn 0;\n}"),
            ("include", "#include <$0>"),
            ("class", "class $0 {\npublic:\n\t\n};"),
            ("struct", "struct $0 {\n\t\n};"),
            ("forrange", "for (const auto& item : $0) {\n\t\n}"),
            ("cout", "std::cout << $0 << std::endl;"),
        ],
    ),
    ("Java", C_LIKE),
    (
        "Java",
        &[
            ("main", "public static void main(String[] args) {\n\t$0\n}"),
            ("class", "public class $0 {\n\t\n}"),
            ("foreach", "for (var item : $0) {\n\t\n}"),
            ("sout", "System.out.println($0);"),
            ("try", "try {\n\t$0\n} catch (Exception e) {\n\t\n}"),
        ],
    ),
    ("C#", C_LIKE),
    (
        "C#",
        &[
            ("main", "static void Main(string[] args)\n{\n\t$0\n}"),
            ("class", "public class $0\n{\n\t\n}"),
            ("foreach", "foreach (var item in $0)\n{\n\t\n}"),
            ("prop", "public string $0 { get; set; }"),
            ("cw", "Console.WriteLine($0);"),
            ("try", "try\n{\n\t$0\n}\ncatch (Exception e)\n{\n\t\n}"),
        ],
    ),
    (
        "Kotlin",
        &[
            ("main", "fun main() {\n\t$0\n}"),
            ("fun", "fun $0() {\n\t\n}"),
            ("class", "class $0 {\n\t\n}"),
            ("for", "for (item in $0) {\n\t\n}"),
            ("if", "if ($0) {\n\t\n}"),
            ("when", "when ($0) {\n\telse -> {}\n}"),
            ("println", "println($0)"),
        ],
    ),
    (
        "Swift",
        &[
            ("func", "func $0() {\n\t\n}"),
            ("struct", "struct $0 {\n\t\n}"),
            ("class", "class $0 {\n\t\n}"),
            ("for", "for item in $0 {\n\t\n}"),
            ("if", "if $0 {\n\t\n}"),
            ("guard", "guard $0 else {\n\treturn\n}"),
            ("print", "print($0)"),
        ],
    ),
    (
        "PHP",
        &[
            ("function", "function $0() {\n\t\n}"),
            ("class", "class $0 {\n\t\n}"),
            ("foreach", "foreach ($0 as $item) {\n\t\n}"),
            ("if", "if ($0) {\n\t\n}"),
            ("echo", "echo $0;"),
        ],
    ),
    (
        "Ruby",
        &[
            ("def", "def $0\n\t\nend"),
            ("class", "class $0\n\t\nend"),
            ("each", "$0.each do |item|\n\t\nend"),
            ("if", "if $0\n\t\nend"),
            ("puts", "puts $0"),
        ],
    ),
    (
        "Shell",
        &[
            ("if", "if [ $0 ]; then\n\t\nfi"),
            ("for", "for item in $0; do\n\t\ndone"),
            ("while", "while $0; do\n\t\ndone"),
            ("function", "$0() {\n\t\n}"),
            ("case", "case $0 in\n\t*)\n\t\t;;\nesac"),
        ],
    ),
    (
        "SQL",
        &[
            ("select", "SELECT $0 FROM ;"),
            ("insert", "INSERT INTO $0 () VALUES ();"),
            ("update", "UPDATE $0 SET  WHERE ;"),
            ("create", "CREATE TABLE $0 (\n\tid INTEGER PRIMARY KEY\n);"),
        ],
    ),
];

/// The snippets that come with `language`.
fn snippets_of(language: &str) -> impl Iterator<Item = &'static (&'static str, &'static str)> {
    let own = SNIPPETS.iter().filter(move |(name, _)| *name == language);
    own.flat_map(|(_, snippets)| snippets.iter())
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Char index where the word ending at `caret` starts.
pub fn word_start(text: &str, caret: usize) -> usize {
    let before = &text[..byte_index(text, caret)];
    caret
        - before
            .chars()
            .rev()
            .take_while(|ch| is_word_char(*ch))
            .count()
}

/// What the word ending at `caret` could become, best first. Empty when no word is there.
pub fn complete(text: &str, caret: usize, language: &Language) -> Vec<Completion> {
    let end = byte_index(text, caret);
    let prefix = &text[byte_index(text, word_start(text, caret))..end];
    if prefix.is_empty() || prefix.starts_with(|ch: char| ch.is_ascii_digit()) {
        return Vec::new();
    }
    let wanted = prefix.to_lowercase();
    let fits = |label: &str| label.to_lowercase().starts_with(&wanted);
    let completion = |label: &str, body: &str, kind| Completion {
        label: label.to_owned(),
        body: body.to_owned(),
        kind,
    };

    // The name being written with its hyphens: of `font-s`, the word is `s` alone.
    let before = &text[..end];
    let named = before
        .chars()
        .rev()
        .take_while(|ch| is_word_char(*ch) || *ch == '-');
    let full = &before[end - named.map(char::len_utf8).sum::<usize>()..];
    let lead = full.len() - prefix.len();
    // Those of `names` the name being written starts, each written from the word on.
    let offered = |names: &mut dyn Iterator<Item = &'static str>, body: &dyn Fn(&str) -> String| {
        let wanted = full.to_lowercase();
        let fitting = names.filter(|name| name.len() > lead && name.starts_with(&wanted));
        let made = fitting.map(|name| completion(name, &body(name)[lead..], Kind::Keyword));
        made.take(MAX_COMPLETIONS).collect::<Vec<_>>()
    };
    let around = &before[..end - full.len()];
    let place = match language.is_markup() {
        true => place_in_markup(around),
        false if language.name == "CSS" => Some(Place::Style(around, false)),
        false => None,
    };
    let found = match place {
        // In a tag: the attributes of its element, then those of any element.
        Some(Place::Attribute(element)) => {
            let own = ATTRIBUTES.iter().filter(|(of, _)| *of == element);
            let known = own.chain(&ATTRIBUTES[..1]);
            let mut names = known.flat_map(|(_, names)| names.iter().copied());
            // One that is there or not takes no value.
            offered(&mut names, &|name| match SWITCHES.contains(&name) {
                true => name.to_owned(),
                false => format!("{name}=\"$0\""),
            })
        }
        Some(Place::Value(attribute)) => {
            let own = ATTRIBUTE_VALUES.iter().filter(|(of, _)| *of == attribute);
            let mut values = own.flat_map(|(_, values)| values.iter().copied());
            offered(&mut values, &str::to_owned)
        }
        // In a style sheet: a property where a declaration starts, its values after it.
        Some(Place::Style(sheet, inline)) => match declaration(sheet, inline) {
            Some(None) => {
                let mut names = CSS_PROPERTIES.iter().copied();
                offered(&mut names, &|name| format!("{name}: $0;"))
            }
            Some(Some(property)) => {
                let own = CSS_VALUES.iter().filter(|(of, _)| *of == property);
                let known = own.chain(&CSS_VALUES[..1]);
                let mut values = known.flat_map(|(_, values)| values.iter().copied());
                offered(&mut values, &str::to_owned)
            }
            None => Vec::new(),
        },
        None => Vec::new(),
    };
    if !found.is_empty() {
        return found;
    }

    let mut found: Vec<Completion> = snippets_of(language.name)
        .filter(|(label, _)| fits(label))
        .map(|(label, body)| completion(label, body, Kind::Snippet))
        .collect();
    // The one named by what is typed comes first: Tab then writes it.
    found.sort_by_key(|snippet| !snippet.label.eq_ignore_ascii_case(prefix));

    // A word typed in full has nothing left to complete.
    let mut seen: HashSet<&str> = HashSet::from([prefix]);
    for keyword in language.keywords() {
        if fits(keyword) && seen.insert(keyword) {
            found.push(completion(keyword, keyword, Kind::Keyword));
        }
    }
    let mut words: Vec<&str> = text
        .split(|ch: char| !is_word_char(ch))
        .filter(|word| word.chars().count() >= MIN_WORD_CHARS && fits(word))
        .filter(|word| seen.insert(word))
        .collect();
    words.sort_by_key(|word| (word.len(), *word));
    found.extend(
        words
            .into_iter()
            .map(|word| completion(word, word, Kind::Word)),
    );
    found.truncate(MAX_COMPLETIONS);
    found
}

/// The attributes of the elements, by element; the first are those of all of them.
const ATTRIBUTES: &[(&str, &[&str])] = &[
    (
        "",
        &[
            "class",
            "id",
            "style",
            "title",
            "hidden",
            "lang",
            "tabindex",
            "role",
            "aria-label",
            "data-",
        ],
    ),
    ("a", &["href", "target", "rel", "download"]),
    ("img", &["src", "alt", "width", "height", "loading"]),
    (
        "input",
        &[
            "type",
            "name",
            "value",
            "placeholder",
            "required",
            "disabled",
            "checked",
            "min",
            "max",
        ],
    ),
    ("form", &["action", "method"]),
    ("button", &["type", "disabled"]),
    ("link", &["rel", "href"]),
    ("script", &["src", "type", "defer", "async"]),
    ("meta", &["name", "content", "charset"]),
    ("label", &["for"]),
    ("select", &["name", "required", "disabled"]),
    (
        "textarea",
        &["name", "rows", "cols", "placeholder", "required"],
    ),
    ("option", &["value", "selected"]),
    ("td", &["colspan", "rowspan"]),
    ("th", &["colspan", "rowspan", "scope"]),
    ("iframe", &["src", "width", "height"]),
    (
        "video",
        &["src", "controls", "autoplay", "loop", "muted", "poster"],
    ),
    ("audio", &["src", "controls", "autoplay", "loop", "muted"]),
    ("source", &["src", "type"]),
];

/// The attributes that are there or not, and take no value.
const SWITCHES: [&str; 12] = [
    "hidden", "required", "disabled", "checked", "selected", "defer", "async", "controls",
    "autoplay", "loop", "muted", "download",
];

/// The properties of a style sheet most often written.
const CSS_PROPERTIES: [&str; 62] = [
    "align-items",
    "align-self",
    "background",
    "background-color",
    "background-image",
    "border",
    "border-bottom",
    "border-color",
    "border-left",
    "border-radius",
    "border-right",
    "border-top",
    "bottom",
    "box-shadow",
    "box-sizing",
    "color",
    "cursor",
    "display",
    "flex",
    "flex-direction",
    "flex-wrap",
    "float",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "gap",
    "grid-template-columns",
    "grid-template-rows",
    "height",
    "justify-content",
    "left",
    "letter-spacing",
    "line-height",
    "list-style",
    "margin",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "max-height",
    "max-width",
    "min-height",
    "min-width",
    "opacity",
    "outline",
    "overflow",
    "padding",
    "padding-bottom",
    "padding-left",
    "padding-right",
    "padding-top",
    "position",
    "right",
    "text-align",
    "text-decoration",
    "text-transform",
    "top",
    "transform",
    "transition",
    "width",
    "z-index",
];

/// The words a property takes as values; the first are those any of them takes.
const CSS_VALUES: &[(&str, &[&str])] = &[
    ("", &["inherit", "initial", "unset", "auto", "none"]),
    (
        "display",
        &[
            "block",
            "inline",
            "inline-block",
            "flex",
            "grid",
            "inline-flex",
            "contents",
        ],
    ),
    (
        "position",
        &["static", "relative", "absolute", "fixed", "sticky"],
    ),
    (
        "flex-direction",
        &["row", "column", "row-reverse", "column-reverse"],
    ),
    ("flex-wrap", &["wrap", "nowrap"]),
    (
        "justify-content",
        &[
            "center",
            "flex-start",
            "flex-end",
            "space-between",
            "space-around",
            "space-evenly",
        ],
    ),
    (
        "align-items",
        &["center", "flex-start", "flex-end", "stretch", "baseline"],
    ),
    (
        "align-self",
        &["center", "flex-start", "flex-end", "stretch", "baseline"],
    ),
    ("text-align", &["left", "right", "center", "justify"]),
    (
        "text-decoration",
        &["underline", "line-through", "overline"],
    ),
    ("text-transform", &["uppercase", "lowercase", "capitalize"]),
    ("font-weight", &["bold", "normal", "lighter", "bolder"]),
    ("font-style", &["italic", "normal", "oblique"]),
    ("overflow", &["hidden", "visible", "scroll"]),
    (
        "cursor",
        &["pointer", "default", "text", "move", "not-allowed", "grab"],
    ),
    ("box-sizing", &["border-box", "content-box"]),
    ("float", &["left", "right"]),
    ("list-style", &["disc", "circle", "square", "decimal"]),
];

/// The values some attributes take.
const ATTRIBUTE_VALUES: &[(&str, &[&str])] = &[
    (
        "type",
        &[
            "text", "password", "email", "number", "checkbox", "radio", "submit", "button",
            "reset", "file", "date", "color", "range", "hidden", "search", "tel", "url",
        ],
    ),
    ("target", &["_blank", "_self", "_parent", "_top"]),
    (
        "rel",
        &["stylesheet", "icon", "noopener", "noreferrer", "nofollow"],
    ),
    ("method", &["get", "post"]),
    ("loading", &["lazy", "eager"]),
    ("charset", &["utf-8"]),
    ("scope", &["col", "row"]),
];

/// Where a name is being written in markup.
enum Place<'a> {
    /// In the tag of this element, where its attributes go.
    Attribute(&'a str),
    /// Between the quotes of the value of this attribute.
    Value(&'a str),
    /// In a style sheet, of which this is the part before the name: the content of a
    /// `style` element, or of a `style` attribute (`true`), which is one block.
    Style(&'a str, bool),
}

/// Where the name that follows `before` is being written, when that says what it can be.
fn place_in_markup(before: &str) -> Option<Place<'_>> {
    let tag = before.rfind('<').map(|at| &before[at + 1..]);
    if let Some(tag) = tag.filter(|tag| !tag.contains('>')) {
        if tag.starts_with(['/', '!', '?']) {
            return None;
        }
        // Its name is written once something follows it.
        let (name, rest) = tag.split_once(char::is_whitespace)?;
        if name.is_empty() || !name.chars().all(in_tag_name) {
            return None;
        }
        if rest.matches('"').count() % 2 == 0 {
            return Some(Place::Attribute(name));
        }
        let quote = rest.rfind('"')?;
        let attribute = rest[..quote].strip_suffix('=')?;
        let attribute = attribute.rsplit(char::is_whitespace).next()?;
        return Some(match attribute {
            "style" => Place::Style(&rest[quote + 1..], true),
            attribute => Place::Value(attribute),
        });
    }
    Some(Place::Style(style_sheet(before)?, false))
}

/// The style sheet `before`, a page up to some place, ends in: what its `style` element
/// holds so far. `None` outside such an element.
pub fn style_sheet(before: &str) -> Option<&str> {
    let sheet = &before[before.rfind("<style")?..];
    if sheet.contains("</style") {
        return None;
    }
    Some(&sheet[sheet.find('>')? + 1..])
}

/// What is being written at the end of `sheet`, a style sheet up to a name: a property
/// (`Some(None)`), or a value of the property named; `None` outside a block, where
/// selectors are. `inline`: the sheet is one block, as in a `style` attribute.
fn declaration(sheet: &str, inline: bool) -> Option<Option<&str>> {
    let in_block = inline || sheet.matches('{').count() > sheet.matches('}').count();
    if !in_block {
        return None;
    }
    let started = sheet.rsplit(['{', ';', '\n']).next().unwrap_or("").trim();
    if started.is_empty() {
        return Some(None);
    }
    let (property, _) = started.split_once(':')?;
    Some(Some(property.trim()))
}

/// A tag of a text: where its name is, in bytes, whether it closes an element, and
/// whether it holds nothing and so goes with no other.
struct Tag {
    name: Range<usize>,
    closes: bool,
    alone: bool,
    /// It is written to its end: one still being typed goes with no other yet.
    whole: bool,
}

fn tags_of(text: &str) -> Vec<Tag> {
    let found = text.match_indices('<').filter_map(|(at, _)| {
        let rest = &text[at + 1..];
        let closes = rest.starts_with('/');
        let start = at + 1 + usize::from(closes);
        let length: usize = text[start..]
            .chars()
            .take_while(|ch| in_tag_name(*ch))
            .map(char::len_utf8)
            .sum();
        let name = &text[start..start + length];
        // `a < b` is no tag; `</>` is one whose name was taken out.
        let after = text[start + length..].chars().next();
        let tag = after.is_some_and(|ch| ch == '>' || ch == '/' || ch.is_whitespace());
        if !tag || (name.is_empty() && !closes && after != Some('>')) {
            return None;
        }
        let end = rest
            .find(['<', '>'])
            .filter(|end| rest[*end..].starts_with('>'));
        let alone = end.is_some_and(|end| rest[..end].ends_with('/'))
            || crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str());
        Some(Tag {
            name: start..start + length,
            closes,
            alone: alone && !closes,
            whole: end.is_some(),
        })
    });
    found.collect()
}

/// Whether one name is the other with one char more.
fn one_char_apart(one: &str, other: &str) -> bool {
    let (long, short) = match one.len() > other.len() {
        true => (one, other),
        false => (other, one),
    };
    let same = long.chars().zip(short.chars()).take_while(|(a, b)| a == b);
    let prefix: usize = same.map(|(ch, _)| ch.len_utf8()).sum();
    let skipped = long[prefix..].chars().next().map_or(0, char::len_utf8);
    long.len() != short.len() && long[prefix + skipped..] == short[prefix..]
}

/// After the name of a tag was edited at the byte `caret`: where the name of the tag that
/// goes with it is, and the name to write there for the two to go together again. `None`
/// when the caret is in no name, or the tag that would go with it bore another name.
pub fn twin_tag(text: &str, caret: usize) -> Option<(Range<usize>, String)> {
    let all = tags_of(text);
    let at = all
        .iter()
        .position(|tag| tag.name.start <= caret && caret <= tag.name.end)?;
    let edited = &all[at];
    if edited.alone || !edited.whole {
        return None;
    }
    // The tag that goes with it: the first one, the way it looks, that pairs with none
    // of those between.
    let mut depth = 0_usize;
    let pairs = |tag: &&Tag| {
        if tag.alone || !tag.whole {
            return false;
        }
        if tag.closes == edited.closes {
            depth += 1;
            return false;
        }
        let found = depth == 0;
        depth = depth.saturating_sub(1);
        found
    };
    let twin = match edited.closes {
        false => all[at + 1..].iter().find(pairs),
        true => all[..at].iter().rev().find(pairs),
    }?;
    let (name, other) = (&text[edited.name.clone()], &text[twin.name.clone()]);
    one_char_apart(name, other).then(|| (twin.name.clone(), name.to_owned()))
}

/// What a tag may be named with.
fn in_tag_name(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '-' | ':' | '.')
}

/// The tags named `name` in `text`, in order: `true` for one that opens an element, `false`
/// for one that closes it. A tag that closes itself (`<a />`) is neither.
fn tags_named<'a>(text: &'a str, name: &'a str) -> impl Iterator<Item = bool> + 'a {
    text.match_indices('<').filter_map(move |(at, _)| {
        let rest = &text[at + 1..];
        let (closes, rest) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let found = rest.get(..name.len())?;
        let longer = rest[name.len()..].chars().next().is_some_and(in_tag_name);
        if !found.eq_ignore_ascii_case(name) || longer {
            return None;
        }
        let empty = rest.find('>').is_some_and(|end| rest[..end].ends_with('/'));
        (closes || !empty).then_some(!closes)
    })
}

/// The closing tag to write after a `>` typed between `before` and `after`, when that `>`
/// ends an opening tag that wants one: not when a closing tag after it is already waiting
/// for it.
pub fn closing_tag(before: &str, after: &str) -> Option<String> {
    let opened = before.rfind('<')?;
    let tag = &before[opened + 1..];
    if tag.contains('>') || tag.ends_with('/') {
        return None;
    }
    let name: String = tag.chars().take_while(|ch| in_tag_name(*ch)).collect();
    let void = crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str());
    if name.is_empty() || void {
        return None;
    }
    // Elements of that name still open before this one: each has a closing tag coming.
    let open = tags_named(&before[..opened], &name).fold(0_usize, |open, opens| match opens {
        true => open + 1,
        false => open.saturating_sub(1),
    });
    // Closing tags after it that close nothing opened there: one more of them than of
    // those elements, and it is the one this tag was missing.
    let (mut depth, mut spare) = (0_usize, 0_usize);
    for opens in tags_named(after, &name) {
        match (opens, depth) {
            (true, _) => depth += 1,
            (false, 0) => spare += 1,
            (false, _) => depth -= 1,
        }
    }
    (spare <= open).then(|| format!("</{name}>"))
}

/// The element still open at the end of `before`: its name, and the byte its opening tag
/// starts at.
pub fn unclosed_tag(before: &str) -> Option<(String, usize)> {
    let mut open: Vec<(String, usize)> = Vec::new();
    for (at, _) in before.match_indices('<') {
        let rest = &before[at + 1..];
        let (closes, rest) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let name: String = rest.chars().take_while(|ch| in_tag_name(*ch)).collect();
        // A tag still being written closes and opens nothing yet.
        let Some(end) = rest.find('>').filter(|_| !name.is_empty()) else {
            continue;
        };
        let same = |(other, _): &(String, usize)| other.eq_ignore_ascii_case(&name);
        if closes {
            // What was left open inside it is closed with it.
            if let Some(found) = open.iter().rposition(same) {
                open.truncate(found);
            }
        } else if !rest[..end].ends_with('/')
            && !crate::format::VOID_TAGS.contains(&name.to_ascii_lowercase().as_str())
        {
            open.push((name, at));
        }
    }
    open.pop()
}

/// Replace the word ending at `caret` by `completion`. Returns the new caret.
pub fn apply(text: &mut String, caret: usize, completion: &Completion, unit: &str) -> usize {
    let start = word_start(text, caret);
    write_snippet(text, start..caret, &completion.body, unit)
}

/// The body of the snippet `label` that comes with `language`.
pub(crate) fn builtin(language: &str, label: &str) -> Option<&'static str> {
    let (_, body) = snippets_of(language).find(|(found, _)| *found == label)?;
    Some(body)
}

/// Write the snippet `body` over the chars `chars`: a tab in it is one `unit` of indent,
/// its lines continue the indent of the line it starts on, `$0` is where the caret goes.
/// Returns the caret.
pub fn write_snippet(text: &mut String, chars: Range<usize>, body: &str, unit: &str) -> usize {
    let bytes = byte_index(text, chars.start)..byte_index(text, chars.end);
    let line_start = text[..bytes.start].rfind('\n').map_or(0, |index| index + 1);
    let lead: String = text[line_start..]
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect();
    let body = body.replace('\t', unit).replace('\n', &format!("\n{lead}"));
    let (body, offset) = match body.find("$0") {
        Some(at) => (body.replacen("$0", "", 1), body[..at].chars().count()),
        None => {
            let len = body.chars().count();
            (body, len)
        }
    };
    text.replace_range(bytes, &body);
    chars.start + offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn labels(text: &str, file: &str) -> Vec<(String, Kind)> {
        complete(text, text.chars().count(), Language::of(Path::new(file)))
            .into_iter()
            .map(|completion| (completion.label, completion.kind))
            .collect()
    }

    #[test]
    fn snippets_keywords_then_words_are_offered() {
        let found = labels("let matcher = 1;\nlet mate = 2;\nma", "a.rs");
        assert_eq!(
            found,
            [
                ("match".to_owned(), Kind::Snippet),
                ("main".to_owned(), Kind::Snippet),
                ("match".to_owned(), Kind::Keyword),
                ("mate".to_owned(), Kind::Word),
                ("matcher".to_owned(), Kind::Word),
            ]
        );
    }

    #[test]
    fn a_word_typed_in_full_still_offers_the_structure_it_names() {
        // `main` typed whole: its snippet is the first, and writes the function.
        for (file, opening) in [
            ("a.rs", "fn main() {"),
            ("a.py", "def main():"),
            ("a.go", "func main() {"),
            ("a.c", "int main("),
            ("a.java", "public static void main("),
            ("a.js", "function main() {"),
        ] {
            let language = Language::of(Path::new(file));
            let found = complete("main", 4, language);
            let first = found.first().expect(file);
            assert_eq!(
                (first.label.as_str(), first.kind),
                ("main", Kind::Snippet),
                "{file}"
            );
            assert!(first.body.contains(opening), "{file}: {}", first.body);
            let mut text = "main".to_owned();
            let caret = apply(&mut text, 4, first, "    ");
            assert!(
                text.contains(opening) && !text.contains("$0"),
                "{file}: {text}"
            );
            assert!(caret <= text.chars().count(), "{file}");
        }
        // Of two that fit, the one typed whole is first.
        let found = complete("if", 2, Language::of(Path::new("a.rs")));
        let labels: Vec<&str> = found.iter().map(|found| found.label.as_str()).collect();
        assert_eq!(labels[..3], ["if", "iflet", "ifelse"]);
        // TypeScript has what JavaScript has, and its own.
        let typed = complete("inter", 5, Language::of(Path::new("a.ts")));
        assert_eq!(
            typed.first().map(|found| found.label.as_str()),
            Some("interface")
        );
    }

    #[test]
    fn nothing_is_offered_without_a_word_or_for_a_finished_one() {
        assert!(labels("total = 1\n", "a.py").is_empty());
        assert!(labels("total = 12", "a.py").is_empty());
        assert!(labels("total = 1\ntotal", "a.py").is_empty());
        assert_eq!(
            labels("étage = 1\nét", "a.py"),
            [("étage".to_owned(), Kind::Word)]
        );
    }

    #[test]
    fn a_snippet_is_indented_and_places_the_caret() {
        let mut text = "class A:\n    de".to_owned();
        let found = complete(&text, 15, Language::of(Path::new("a.py")));
        assert_eq!(found[0].label, "def");
        let caret = apply(&mut text, 15, &found[0], "    ");
        assert_eq!(text, "class A:\n    def ():\n        pass");
        assert_eq!(caret, 17);
    }

    #[test]
    fn an_opening_tag_gets_its_closing_tag() {
        assert_eq!(
            closing_tag("<div class=\"a\"", "").as_deref(),
            Some("</div>")
        );
        assert_eq!(closing_tag("<p>text <b", "</p>").as_deref(), Some("</b>"));
        let none = [
            ("<br", ""),
            ("</div", ""),
            ("<img /", ""),
            ("a ", ""),
            ("<b", "</b>"),
        ];
        for (before, after) in none {
            assert_eq!(closing_tag(before, after), None, "{before}");
        }
        // A closing tag already there, further down, is the one: no second one is written.
        assert_eq!(
            closing_tag("<html>\n<body", "\n<p>a</p>\n</body>\n</html>"),
            None
        );
        assert_eq!(closing_tag("<BODY", "\n</body>"), None);
        // One that closes an element opened before is not for this tag.
        assert_eq!(
            closing_tag("<div>\n  <div", "\n</div>").as_deref(),
            Some("</div>")
        );
        assert_eq!(closing_tag("<div>\n  <div", "\n  </div>\n</div>"), None);
        // Nor is one that closes an element opened after it.
        assert_eq!(closing_tag("<p", "<p>a</p>").as_deref(), Some("</p>"));
        // A name that only starts the same is another element.
        assert_eq!(closing_tag("<b", "</body>").as_deref(), Some("</b>"));
    }

    #[test]
    fn attributes_and_styles_are_offered_where_they_go() {
        let offered = |text: &str, file: &str| {
            let found = complete(text, text.chars().count(), Language::of(Path::new(file)));
            found
                .into_iter()
                .map(|completion| completion.body)
                .collect::<Vec<_>>()
        };
        // In a tag: the attributes of its element, then those of any element.
        assert_eq!(offered("<a h", "a.html"), ["href=\"$0\"", "hidden"]);
        assert_eq!(offered("<input class=\"a\" re", "a.html"), ["required"]);
        assert_eq!(offered("<p cl", "a.html"), ["class=\"$0\""]);
        // Not in the value of one, nor outside a tag.
        assert!(!offered("<p class=\"cl", "a.html").contains(&"class=\"$0\"".to_owned()));
        assert!(!offered("<p>cl", "a.html").contains(&"class=\"$0\"".to_owned()));
        // In a style sheet: properties in a block, values after a property.
        assert_eq!(offered("a {\n  disp", "a.css"), ["display: $0;"]);
        assert_eq!(offered("a {\n  display: fl", "a.css"), ["flex"]);
        assert_eq!(offered("a {\n  color: in", "a.css"), ["inherit", "initial"]);
        assert!(!offered("disp", "a.css").contains(&"display: $0;".to_owned()));
        // A name with hyphens is offered whole and written from the word on.
        assert_eq!(
            offered("a {\n  font-s", "a.css"),
            ["size: $0;", "style: $0;"]
        );
        assert_eq!(offered("a { display: inline-b", "a.css"), ["block"]);
        assert_eq!(offered("<p aria-l", "a.html"), ["label=\"$0\""]);
        // In a page: the style element is a style sheet, the style attribute a block.
        assert_eq!(
            offered("<style>\nbody {\n  marg", "a.html"),
            [
                "margin: $0;",
                "margin-bottom: $0;",
                "margin-left: $0;",
                "margin-right: $0;",
                "margin-top: $0;"
            ]
        );
        assert_eq!(
            offered("<p style=\"color: red; disp", "a.html"),
            ["display: $0;"]
        );
        assert_eq!(offered("<p style=\"display: gr", "a.html"), ["grid"]);
        assert!(
            !offered("<style>a{}</style>\n<p>disp", "a.html").contains(&"display: $0;".to_owned())
        );
        // The values of an attribute, between its quotes.
        assert_eq!(offered("<input type=\"pa", "a.html"), ["password"]);
        assert_eq!(offered("<a target=\"_b", "a.html"), ["_blank"]);
    }

    #[test]
    fn the_tag_that_goes_with_an_edited_one_is_found() {
        let twin = |text: &str, caret: usize| twin_tag(text, caret);
        // A char typed in the name of the opening tag: the closing one takes the name.
        assert_eq!(
            twin("<divx><p>a</p></div>", 5),
            Some((16..19, "divx".to_owned()))
        );
        // A char erased in the closing tag: the opening one follows.
        assert_eq!(
            twin("<div><br>a<img/></di>", 20),
            Some((1..4, "di".to_owned()))
        );
        // Down to no name at all, and back.
        assert_eq!(twin("<><b>x</b></p>", 1), Some((12..13, String::new())));
        assert_eq!(twin("<h><b>x</b></>", 2), Some((13..13, "h".to_owned())));
        // Tags that did not go together are left alone: so are those that already do.
        assert_eq!(twin("<div>a</span>", 3), None);
        assert_eq!(twin("<div>a</div>", 3), None);
        assert_eq!(twin("<br>a", 2), None);
        assert_eq!(twin("a < b", 3), None);
        assert_eq!(twin("<div>a</div>", 5), None);
        // A tag still being typed is the start of a new element, not the old one renamed.
        assert_eq!(
            twin(
                "<div>
<di
</div>",
                9
            ),
            None
        );
        assert_eq!(
            twin(
                "<div>
<di 
</div>",
                9
            ),
            None
        );
        assert!(one_char_apart("div", "dv") && one_char_apart("", "a"));
        assert!(!one_char_apart("div", "div") && !one_char_apart("div", "span"));
    }

    #[test]
    fn the_element_still_open_is_found() {
        let text = "<html>\n  <body>\n    <p>a <b>b</b><br></p>\n    <img src=\"x\" />\n    ";
        assert_eq!(unclosed_tag(text), Some(("body".to_owned(), 9)));
        assert_eq!(unclosed_tag("<ul><li>a"), Some(("li".to_owned(), 4)));
        // A closing tag closes what was left open inside its element too.
        assert_eq!(unclosed_tag("<ul><li>a</ul>"), None);
        assert_eq!(unclosed_tag("<!doctype html>\n"), None);
        assert_eq!(unclosed_tag("<div"), None);
    }

    #[test]
    fn a_word_replaces_what_was_typed() {
        let mut text = "counter = 0\ncou + 1".to_owned();
        let found = complete(&text, 15, Language::of(Path::new("a.py")));
        let caret = apply(&mut text, 15, &found[0], "    ");
        assert_eq!(text, "counter = 0\ncounter + 1");
        assert_eq!(caret, 19);
    }
}
