//! Try a regular expression on a text: every match, its groups, and what a replacement gives.

use std::ops::Range;

use regex::RegexBuilder;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub ignore_case: bool,
    /// `^` and `$` match at each line.
    pub multi_line: bool,
    /// `.` matches a line break too.
    pub dot_all: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// `1`, `2`… or the name of a named group.
    pub name: String,
    /// Byte range in the text; `None` when the group took no part.
    pub range: Option<Range<usize>>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub range: Range<usize>,
    pub text: String,
    pub groups: Vec<Group>,
}

/// At most this many matches are listed.
pub const MAX_FOUND: usize = 1000;

fn build(pattern: &str, flags: Flags) -> Result<regex::Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(flags.ignore_case)
        .multi_line(flags.multi_line)
        .dot_matches_new_line(flags.dot_all)
        .size_limit(10 * 1024 * 1024)
        .build()
        .map_err(|err| err.to_string())
}

pub fn run(pattern: &str, flags: Flags, text: &str) -> Result<Vec<Found>, String> {
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    let regex = build(pattern, flags)?;
    let names: Vec<Option<&str>> = regex.capture_names().collect();
    Ok(regex
        .captures_iter(text)
        .take(MAX_FOUND)
        .map(|captures| {
            let whole = captures.get(0).expect("a match has group 0");
            let groups = (1..captures.len())
                .map(|index| {
                    let group = captures.get(index);
                    Group {
                        name: names[index].map_or_else(|| index.to_string(), str::to_owned),
                        range: group.map(|group| group.range()),
                        text: group.map_or_else(String::new, |group| group.as_str().to_owned()),
                    }
                })
                .collect();
            Found {
                range: whole.range(),
                text: whole.as_str().to_owned(),
                groups,
            }
        })
        .collect())
}

/// The text with every match replaced; `$1`, `${name}` stand for groups.
pub fn replace(pattern: &str, flags: Flags, text: &str, with: &str) -> Result<String, String> {
    Ok(build(pattern, flags)?.replace_all(text, with).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_matches_and_groups() {
        let found = run(
            r"(?P<user>\w+)@(\w+)\.com",
            Flags::default(),
            "ada@math.com, bob@web.com",
        )
        .unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text, "ada@math.com");
        assert_eq!(found[0].groups[0].name, "user");
        assert_eq!(found[0].groups[0].text, "ada");
        assert_eq!(found[1].groups[1].name, "2");
        assert_eq!(found[1].groups[1].text, "web");
        assert_eq!(found[1].range, 14..25);
    }

    #[test]
    fn flags_change_the_matches() {
        let text = "One\none";
        assert_eq!(run("^one$", Flags::default(), text).unwrap().len(), 0);
        let flags = Flags {
            ignore_case: true,
            multi_line: true,
            dot_all: false,
        };
        assert_eq!(run("^one$", flags, text).unwrap().len(), 2);
        let dot_all = Flags {
            dot_all: true,
            ..Flags::default()
        };
        assert_eq!(run("One.one", dot_all, text).unwrap().len(), 1);
    }

    #[test]
    fn replaces_and_reports_errors() {
        let out = replace(r"(\w+)@(\w+)", Flags::default(), "ada@math", "$2:$1").unwrap();
        assert_eq!(out, "math:ada");
        assert!(run("(", Flags::default(), "x").is_err());
        assert!(run("", Flags::default(), "x").unwrap().is_empty());
    }
}
