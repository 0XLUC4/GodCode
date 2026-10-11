//! Abbreviations that write markup, after Emmet: `ul>li.item*3` becomes a list of three
//! items. An element is a name (`div`), with `#id`, `.class`, `[attributes]` and `{text}`;
//! `>` puts what follows inside it, `+` beside it, `*3` writes it three times, a `$` in it
//! being its number. `!` is a whole page.

/// The elements that hold nothing: they have no closing tag.
const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

struct Element {
    name: String,
    id: Option<String>,
    classes: Vec<String>,
    /// Each as written between the brackets: `href=a.html`, or a name alone.
    attributes: Vec<String>,
    text: String,
    times: usize,
    children: Vec<Element>,
}

/// What is left of an abbreviation being read.
struct Reader<'a> {
    rest: &'a str,
}

impl<'a> Reader<'a> {
    fn eat(&mut self, wanted: char) -> bool {
        let rest = self.rest.strip_prefix(wanted);
        rest.inspect(|rest| self.rest = rest).is_some()
    }

    /// The chars at the start that `allowed` takes; none is an empty word.
    fn word(&mut self, allowed: impl Fn(char) -> bool) -> &'a str {
        let end = self.rest.find(|ch| !allowed(ch)).unwrap_or(self.rest.len());
        let (word, rest) = self.rest.split_at(end);
        self.rest = rest;
        word
    }

    /// What stands before `end`, which is taken too.
    fn until(&mut self, end: char) -> Option<&'a str> {
        let (inside, rest) = self.rest.split_once(end)?;
        self.rest = rest;
        Some(inside)
    }
}

/// The element an abbreviation names by its class or id alone, inside `parent`.
fn implied(parent: Option<&str>) -> &'static str {
    match parent {
        Some("ul" | "ol") => "li",
        Some("table" | "tbody" | "thead" | "tfoot") => "tr",
        Some("tr") => "td",
        Some("select") => "option",
        _ => "div",
    }
}

fn element(reader: &mut Reader<'_>, parent: Option<&str>) -> Option<Element> {
    let named = |ch: char| ch.is_ascii_alphanumeric() || matches!(ch, '-' | ':');
    let labelled = |ch: char| ch.is_alphanumeric() || matches!(ch, '-' | '_' | '$');
    let mut element = Element {
        name: reader.word(named).to_owned(),
        id: None,
        classes: Vec::new(),
        attributes: Vec::new(),
        text: String::new(),
        times: 1,
        children: Vec::new(),
    };
    let mut described = false;
    loop {
        if reader.eat('#') {
            let id = reader.word(labelled);
            element.id = (!id.is_empty()).then(|| id.to_owned());
            element.id.as_ref()?;
        } else if reader.eat('.') {
            let class = reader.word(labelled);
            if class.is_empty() {
                return None;
            }
            element.classes.push(class.to_owned());
        } else if reader.eat('[') {
            let inside = reader.until(']')?;
            let written = inside.split_whitespace().map(str::to_owned);
            element.attributes.extend(written);
        } else if reader.eat('{') {
            reader.until('}')?.clone_into(&mut element.text);
        } else {
            break;
        }
        described = true;
    }
    if reader.eat('*') {
        let times = reader.word(|ch| ch.is_ascii_digit()).parse().ok();
        element.times = times.filter(|times| (1..=200).contains(times))?;
    }
    if element.name.is_empty() {
        // Nothing at all is no element.
        if !described {
            return None;
        }
        implied(parent).clone_into(&mut element.name);
    }
    element
        .name
        .starts_with(|ch: char| ch.is_ascii_alphabetic())
        .then_some(element)
}

/// Elements side by side; the last one may hold others.
fn siblings(reader: &mut Reader<'_>, parent: Option<&str>) -> Option<Vec<Element>> {
    let mut found = Vec::new();
    loop {
        let mut next = element(reader, parent)?;
        if reader.eat('>') {
            let name = next.name.clone();
            next.children = siblings(reader, Some(&name))?;
            found.push(next);
            return Some(found);
        }
        found.push(next);
        if !reader.eat('+') {
            return Some(found);
        }
    }
}

/// The attributes an element is of no use without, when they were not given.
fn expected(name: &str) -> &'static [&'static str] {
    match name {
        "a" => &["href"],
        "img" => &["src", "alt"],
        "link" => &["rel=stylesheet", "href"],
        "script" => &[],
        "input" => &["type=text"],
        "form" => &["action"],
        "label" => &["for"],
        "iframe" => &["src"],
        _ => &[],
    }
}

/// Write `elements`, one a line, `depth` levels in. A tab is one level.
fn write(elements: &[Element], depth: usize, out: &mut String) {
    for element in elements {
        for number in 1..=element.times {
            let numbered = |written: &str| written.replace('$', &number.to_string());
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&"\t".repeat(depth));
            out.push('<');
            out.push_str(&element.name);
            if let Some(id) = &element.id {
                out.push_str(&format!(" id=\"{}\"", numbered(id)));
            }
            if !element.classes.is_empty() {
                out.push_str(&format!(
                    " class=\"{}\"",
                    numbered(&element.classes.join(" "))
                ));
            }
            let given = element.attributes.iter().map(String::as_str);
            let named = |attribute: &str| attribute.split('=').next().unwrap_or("").to_owned();
            let missing = expected(&element.name).iter().copied().filter(|wanted| {
                !element
                    .attributes
                    .iter()
                    .any(|given| named(given) == named(wanted))
            });
            for attribute in given.chain(missing) {
                let (name, value) = attribute.split_once('=').unwrap_or((attribute, ""));
                let value = value.trim_matches(['"', '\'']);
                out.push_str(&format!(" {name}=\"{}\"", numbered(value)));
            }
            out.push('>');
            if VOID.contains(&element.name.as_str()) {
                continue;
            }
            out.push_str(&numbered(&element.text));
            if !element.children.is_empty() {
                write(&element.children, depth + 1, out);
                out.push('\n');
                out.push_str(&"\t".repeat(depth));
            }
            out.push_str(&format!("</{}>", element.name));
        }
    }
}

/// What `abbreviation` stands for, as a snippet: a tab is one level of indent, `$0` is
/// where the caret goes. `tags`: the names of the elements there are; a word alone is an
/// abbreviation only when it is one of them. `None` when it is no abbreviation.
pub fn expand(abbreviation: &str, tags: &[&str]) -> Option<String> {
    if abbreviation == "!" {
        return crate::complete::builtin("HTML", "html5").map(str::to_owned);
    }
    let mut reader = Reader { rest: abbreviation };
    let elements = siblings(&mut reader, None)?;
    if !reader.rest.is_empty() {
        return None;
    }
    let word = abbreviation.chars().all(|ch| ch.is_ascii_alphanumeric());
    if word && !tags.contains(&abbreviation) {
        return None;
    }
    let mut out = String::new();
    write(&elements, 0, &mut out);
    // The caret goes to the first thing left to fill in: an attribute, or an element.
    let empty = [
        out.find("=\"\"").map(|at| at + 2),
        out.find("></").map(|at| at + 1),
    ];
    let caret = empty.into_iter().flatten().min().unwrap_or(out.len());
    out.insert_str(caret, "$0");
    Some(out)
}

/// Abbreviations of a style sheet that are a whole declaration.
const CSS_WORDS: [(&str, &str); 34] = [
    ("df", "display: flex"),
    ("dg", "display: grid"),
    ("db", "display: block"),
    ("di", "display: inline"),
    ("dib", "display: inline-block"),
    ("dn", "display: none"),
    ("posr", "position: relative"),
    ("posa", "position: absolute"),
    ("posf", "position: fixed"),
    ("poss", "position: sticky"),
    ("tac", "text-align: center"),
    ("tal", "text-align: left"),
    ("tar", "text-align: right"),
    ("jcc", "justify-content: center"),
    ("jcsb", "justify-content: space-between"),
    ("jcsa", "justify-content: space-around"),
    ("jcfs", "justify-content: flex-start"),
    ("jcfe", "justify-content: flex-end"),
    ("aic", "align-items: center"),
    ("aifs", "align-items: flex-start"),
    ("aife", "align-items: flex-end"),
    ("ais", "align-items: stretch"),
    ("fdc", "flex-direction: column"),
    ("fdr", "flex-direction: row"),
    ("fww", "flex-wrap: wrap"),
    ("fwb", "font-weight: bold"),
    ("fsi", "font-style: italic"),
    ("tdn", "text-decoration: none"),
    ("tdu", "text-decoration: underline"),
    ("ttu", "text-transform: uppercase"),
    ("curp", "cursor: pointer"),
    ("ovh", "overflow: hidden"),
    ("ova", "overflow: auto"),
    ("bxz", "box-sizing: border-box"),
];

/// Abbreviations of the properties that take sizes or a colour, with the unit a bare
/// number gets: none for those that count rather than measure.
const CSS_SIZES: [(&str, &str, &str); 30] = [
    ("m", "margin", "px"),
    ("mt", "margin-top", "px"),
    ("mr", "margin-right", "px"),
    ("mb", "margin-bottom", "px"),
    ("ml", "margin-left", "px"),
    ("p", "padding", "px"),
    ("pt", "padding-top", "px"),
    ("pr", "padding-right", "px"),
    ("pb", "padding-bottom", "px"),
    ("pl", "padding-left", "px"),
    ("w", "width", "px"),
    ("h", "height", "px"),
    ("maw", "max-width", "px"),
    ("mah", "max-height", "px"),
    ("miw", "min-width", "px"),
    ("mih", "min-height", "px"),
    ("t", "top", "px"),
    ("r", "right", "px"),
    ("b", "bottom", "px"),
    ("l", "left", "px"),
    ("g", "gap", "px"),
    ("fz", "font-size", "px"),
    ("bdrs", "border-radius", "px"),
    ("fw", "font-weight", ""),
    ("lh", "line-height", ""),
    ("z", "z-index", ""),
    ("op", "opacity", ""),
    ("fx", "flex", ""),
    ("c", "color", ""),
    ("bgc", "background-color", ""),
];

/// A size as abbreviated: `10` in the unit of its property, `50p` a percentage, `1.5e` in
/// em, `2r` in rem, `a` for auto.
fn css_size(written: &str, unit: &str) -> Option<String> {
    if written == "a" {
        return Some("auto".to_owned());
    }
    let digits = written.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.');
    let (number, suffix) = written.split_at(written.len() - digits.len());
    number.parse::<f32>().ok()?;
    let unit = match suffix {
        "" => unit,
        "p" | "%" => "%",
        "e" => "em",
        "r" => "rem",
        "px" | "em" | "rem" | "vh" | "vw" => suffix,
        _ => return None,
    };
    // Nothing is nothing in any unit.
    Some(match number {
        "0" => "0".to_owned(),
        number => format!("{number}{unit}"),
    })
}

/// What `abbreviation` stands for in a block of a style sheet, as a snippet: `m10-20` is
/// `margin: 10px 20px;`, `df` is `display: flex;`, `c#333` a colour, `w` alone a width
/// left to write. `None` when it is no abbreviation.
pub fn expand_css(abbreviation: &str) -> Option<String> {
    if let Some((_, whole)) = CSS_WORDS.iter().find(|(short, _)| *short == abbreviation) {
        return Some(format!("{whole};"));
    }
    let value = abbreviation.trim_start_matches(|ch: char| ch.is_ascii_lowercase());
    let short = &abbreviation[..abbreviation.len() - value.len()];
    let (_, property, unit) = CSS_SIZES.iter().find(|(found, ..)| *found == short)?;
    if value.is_empty() {
        return Some(format!("{property}: $0;"));
    }
    if let Some(colour) = value.strip_prefix('#') {
        let hexadecimal = colour.chars().all(|ch| ch.is_ascii_hexdigit());
        let whole = hexadecimal && matches!(colour.len(), 3 | 4 | 6 | 8);
        return whole.then(|| format!("{property}: #{colour};"));
    }
    let sizes: Option<Vec<String>> = value.split('-').map(|size| css_size(size, unit)).collect();
    Some(format!("{property}: {};", sizes?.join(" ")))
}

/// The abbreviation `before`, the start of a line up to the caret, ends with: what follows
/// the last white space, or the last tag. Text between braces may hold spaces.
pub fn abbreviation(before: &str) -> Option<&str> {
    // Inside a tag still being written, what is typed is the tag.
    if let Some(tag) = before.rfind('<') {
        before[tag..].find('>')?;
    }
    let mut depth = 0_usize;
    let mut start = 0;
    for (at, ch) in before.char_indices().rev() {
        match ch {
            '}' | ']' => depth += 1,
            '{' | '[' => depth = depth.saturating_sub(1),
            ch if ch.is_whitespace() && depth == 0 => {
                start = at + ch.len_utf8();
                break;
            }
            _ => {}
        }
    }
    let mut found = &before[start..];
    // After a tag: the `>` that ends it is not the one that puts an element in another.
    if let Some(tag) = found.rfind('<') {
        let end = found[tag..].find('>')?;
        found = &found[tag + end + 1..];
    }
    (!found.is_empty()).then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAGS: [&str; 6] = ["div", "p", "ul", "li", "a", "img"];

    fn expanded(abbreviation: &str) -> Option<String> {
        expand(abbreviation, &TAGS)
    }

    #[test]
    fn an_abbreviation_writes_its_elements() {
        assert_eq!(expanded("div").as_deref(), Some("<div>$0</div>"));
        assert_eq!(
            expanded("div#main.a.b").as_deref(),
            Some("<div id=\"main\" class=\"a b\">$0</div>")
        );
        assert_eq!(
            expanded(".box").as_deref(),
            Some("<div class=\"box\">$0</div>")
        );
        assert_eq!(
            expanded("ul>li*2").as_deref(),
            Some("<ul>\n\t<li>$0</li>\n\t<li></li>\n</ul>")
        );
        assert_eq!(
            expanded("ul>.item$*2").as_deref(),
            Some("<ul>\n\t<li class=\"item1\">$0</li>\n\t<li class=\"item2\"></li>\n</ul>")
        );
        assert_eq!(
            expanded("p{Hello you}+a").as_deref(),
            Some("<p>Hello you</p>\n<a href=\"$0\"></a>")
        );
        assert_eq!(
            expanded("a[href=page.html target=_blank]{Go}").as_deref(),
            Some("<a href=\"page.html\" target=\"_blank\">Go</a>$0")
        );
        assert_eq!(
            expanded("img").as_deref(),
            Some("<img src=\"$0\" alt=\"\">")
        );
        assert_eq!(
            expanded("div>p+ul>li").as_deref(),
            Some("<div>\n\t<p>$0</p>\n\t<ul>\n\t\t<li></li>\n\t</ul>\n</div>")
        );
        assert!(expanded("!").is_some_and(|page| page.starts_with("<!doctype html>")));
    }

    #[test]
    fn what_is_no_abbreviation_is_left_alone() {
        // A word that names no element is a word; so are numbers and signs.
        for not in [
            "hello", "div>", ">p", "p*0", "p*", "a..b", "#", "p[a", "1", "a>>b", "x=1",
        ] {
            assert_eq!(expanded(not), None, "{not}");
        }
        // With a class or an id, any name is an element.
        assert_eq!(
            expanded("my-card.big").as_deref(),
            Some("<my-card class=\"big\">$0</my-card>")
        );
    }

    #[test]
    fn an_abbreviation_writes_a_declaration_in_a_style_sheet() {
        let written = |abbreviation: &str| expand_css(abbreviation);
        assert_eq!(written("m10").as_deref(), Some("margin: 10px;"));
        assert_eq!(written("p10-20").as_deref(), Some("padding: 10px 20px;"));
        assert_eq!(written("m0-a").as_deref(), Some("margin: 0 auto;"));
        assert_eq!(written("w100p").as_deref(), Some("width: 100%;"));
        assert_eq!(written("fz1.5r").as_deref(), Some("font-size: 1.5rem;"));
        assert_eq!(written("fw700").as_deref(), Some("font-weight: 700;"));
        assert_eq!(written("c#333").as_deref(), Some("color: #333;"));
        assert_eq!(written("df").as_deref(), Some("display: flex;"));
        assert_eq!(
            written("jcsb").as_deref(),
            Some("justify-content: space-between;")
        );
        assert_eq!(written("w").as_deref(), Some("width: $0;"));
        // A selector, a word, a colour that is none: left as they are.
        for not in ["a", "body", "m10x", "c#12", "m-", "10", "color"] {
            assert_eq!(written(not), None, "{not}");
        }
    }

    #[test]
    fn the_abbreviation_before_the_caret_is_found() {
        assert_eq!(abbreviation("    ul>li*3"), Some("ul>li*3"));
        assert_eq!(abbreviation("<div>p.a"), Some("p.a"));
        assert_eq!(abbreviation("text p{a b}+a"), Some("p{a b}+a"));
        assert_eq!(abbreviation("a[href=x title=y]"), Some("a[href=x title=y]"));
        // Inside a tag, or after nothing, there is none.
        assert_eq!(abbreviation("<div cla"), None);
        assert_eq!(abbreviation("    "), None);
        assert_eq!(abbreviation("<p>"), None);
    }
}
