//! Reading what the `git` command prints: the state of a folder, the changed lines of a file.

/// What happened to a file since the last commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

impl State {
    /// The letter Git tools show for this state.
    pub fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Untracked => 'U',
            Self::Conflict => '!',
        }
    }
}

/// A file that differs from the last commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Relative to the root of the repository, with `/` between folders.
    pub path: String,
    pub state: State,
}

/// How lines of a file differ from the last commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChange {
    Added,
    Modified,
    /// Lines were removed just under this one.
    Deleted,
}

/// Lines changed together: the first one (counted from 0), how many, and how.
pub type Hunk = (usize, usize, LineChange);

fn state(code: &str) -> State {
    let has = |letter: char| code.contains(letter);
    if code == "??" {
        State::Untracked
    } else if has('U') || code == "AA" || code == "DD" {
        State::Conflict
    } else if has('D') {
        State::Deleted
    } else if has('R') {
        State::Renamed
    } else if has('A') {
        State::Added
    } else {
        State::Modified
    }
}

/// The branch and the changed files, from `git status --porcelain=v1 -b`.
pub fn parse_status(output: &str) -> (Option<String>, Vec<Change>) {
    let mut branch = None;
    let mut changes = Vec::new();
    for line in output.lines() {
        if let Some(head) = line.strip_prefix("## ") {
            // "main...origin/main [ahead 1]", or "No commits yet on main".
            let name = head.strip_prefix("No commits yet on ").unwrap_or(head);
            let name = name.split("...").next().unwrap_or(name);
            branch = Some(name.split(' ').next().unwrap_or(name).to_owned());
        } else if let (Some(code), Some(path)) = (line.get(..2), line.get(3..)) {
            // A rename reads "old -> new": the new name is the file that exists.
            let path = path.rsplit(" -> ").next().unwrap_or(path);
            changes.push(Change {
                path: path.trim_matches('"').to_owned(),
                state: state(code),
            });
        }
    }
    (branch, changes)
}

/// Start and length of one side of a hunk header: `12,3` or `12`.
fn span(text: &str) -> Option<(usize, usize)> {
    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
    Some((start.parse().ok()?, count.parse().ok()?))
}

/// The changed lines of a file, from `git diff -U0`.
pub fn parse_diff(output: &str) -> Vec<Hunk> {
    output
        .lines()
        .filter_map(|line| {
            // "@@ -old +new @@ context"
            let mut sides = line.strip_prefix("@@ -")?.split(' ');
            let (_, removed) = span(sides.next()?)?;
            let (start, added) = span(sides.next()?.strip_prefix('+')?)?;
            Some(if added == 0 {
                (start.saturating_sub(1), 1, LineChange::Deleted)
            } else if removed == 0 {
                (start - 1, added, LineChange::Added)
            } else {
                (start - 1, added, LineChange::Modified)
            })
        })
        .collect()
}

/// Which side of a conflict to keep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keep {
    /// What this branch had.
    Ours,
    /// What comes from the other branch.
    Theirs,
    Both,
}

/// `text` with the conflict around line `line` (counted from 0) settled, its markers gone;
/// `None` when that line is in no conflict.
pub fn resolve_conflict(text: &str, line: usize, keep: Keep) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let line = line.min(lines.len().checked_sub(1)?);
    let marks = |at: &usize, mark: &str| lines[*at].starts_with(mark);
    let start = (0..=line).rev().find(|at| marks(at, "<<<<<<<"))?;
    let end = (start..lines.len()).find(|at| marks(at, ">>>>>>>"))?;
    let middle = (start..end).find(|at| marks(at, "======="))?;
    if end < line {
        return None;
    }
    // In the three-way style, what both sides started from sits before the `=======`.
    let ours_end = (start..middle)
        .find(|at| marks(at, "|||||||"))
        .unwrap_or(middle);
    let ours = &lines[start + 1..ours_end];
    let theirs = &lines[middle + 1..end];
    let mut settled = lines[..start].to_vec();
    if keep != Keep::Theirs {
        settled.extend(ours);
    }
    if keep != Keep::Ours {
        settled.extend(theirs);
    }
    settled.extend(&lines[end + 1..]);
    Some(settled.join("\n"))
}

/// Who last changed a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blame {
    pub author: String,
    /// As `2026-10-11`.
    pub date: String,
    /// The first line of the message of the commit.
    pub summary: String,
}

/// The day a number of seconds since 1970 falls on, as `2026-10-11`.
fn day_of(seconds: i64) -> String {
    let shifted = seconds.div_euclid(86_400) + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// What `git blame --line-porcelain` printed: one entry per line of the file.
pub fn parse_blame(printed: &str) -> Vec<Blame> {
    let mut lines = Vec::new();
    let mut current = Blame {
        author: String::new(),
        date: String::new(),
        summary: String::new(),
    };
    for line in printed.lines() {
        if line.starts_with('\t') {
            lines.push(current.clone());
        } else if let Some(author) = line.strip_prefix("author ") {
            current.author = author.to_owned();
        } else if let Some(time) = line.strip_prefix("author-time ") {
            current.date = time.parse().map(day_of).unwrap_or_default();
        } else if let Some(summary) = line.strip_prefix("summary ") {
            current.summary = summary.to_owned();
        }
    }
    lines
}

/// A row of a side by side comparison: the line as it was and as it is, with their
/// numbers; one side is missing where a line was added or removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub old: Option<(usize, String)>,
    pub new: Option<(usize, String)>,
}

/// The rows of a unified diff, removed and added lines facing each other. With `titles`,
/// each file starts with a row numbered 0 holding its name on both sides.
pub fn side_by_side(diff: &str, titles: bool) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    // The first of the removed lines still waiting for the added line that replaces it.
    let mut unpaired = 0;
    let (mut old, mut new) = (0, 0);
    let mut started = false;
    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@ -") {
            let start = |part: Option<&str>| {
                let digits = part?.trim_start_matches('+').split(',').next()?;
                digits.parse::<usize>().ok()
            };
            let mut parts = header.split(' ');
            old = start(parts.next()).unwrap_or(1);
            new = start(parts.next()).unwrap_or(1);
            started = true;
            unpaired = rows.len();
            continue;
        }
        if let Some(names) = line.strip_prefix("diff --git a/") {
            started = false;
            let name = names.split(" b/").next().unwrap_or(names).to_owned();
            if titles {
                rows.push(Row {
                    old: Some((0, name.clone())),
                    new: Some((0, name)),
                });
            }
            continue;
        }
        if !started {
            continue;
        }
        let text = line.get(1..).unwrap_or("").to_owned();
        match line.as_bytes().first() {
            Some(b'-') => {
                rows.push(Row {
                    old: Some((old, text)),
                    new: None,
                });
                old += 1;
            }
            // An added line faces the first removed line still alone, when there is one.
            Some(b'+') => {
                match rows.get_mut(unpaired) {
                    Some(row) => row.new = Some((new, text)),
                    None => rows.push(Row {
                        old: None,
                        new: Some((new, text)),
                    }),
                }
                unpaired += 1;
                new += 1;
            }
            Some(b' ') => {
                rows.push(Row {
                    old: Some((old, text.clone())),
                    new: Some((new, text)),
                });
                old += 1;
                new += 1;
                unpaired = rows.len();
            }
            // "\ No newline at end of file" and the like.
            _ => {}
        }
    }
    rows
}

/// The commit a line of `git log` is about, with or without the drawing of the branches
/// before it: its abbreviated hash. A line that only draws has none.
pub fn hash_in(line: &str) -> Option<&str> {
    let hash = |word: &&str| {
        (7..=40).contains(&word.len()) && word.chars().all(|ch| ch.is_ascii_hexdigit())
    };
    line.split_whitespace().find(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commit_of_a_line_of_the_log_is_found() {
        assert_eq!(
            hash_in("a1b2c3d  2026-10-11  Luca  fix: a thing"),
            Some("a1b2c3d")
        );
        assert_eq!(
            hash_in("| * 0f9e8d7  2026-10-10  Luca (main)  merge"),
            Some("0f9e8d7")
        );
        assert_eq!(hash_in(r"|\  "), None);
        assert_eq!(hash_in("* | "), None);
        // A word of the message is no hash, a short number neither.
        assert_eq!(
            hash_in("* deadbeef1  2026-10-09  Ada  add cafe 123"),
            Some("deadbeef1")
        );
        assert_eq!(hash_in("just words here"), None);
    }

    #[test]
    fn a_conflict_is_settled_for_one_side_or_both() {
        let text =
            "top\n<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\ntheirs\n>>>>>>> other\nend";
        let settled = |keep| resolve_conflict(text, 3, keep);
        assert_eq!(settled(Keep::Ours).as_deref(), Some("top\nmine\nend"));
        assert_eq!(settled(Keep::Theirs).as_deref(), Some("top\ntheirs\nend"));
        assert_eq!(
            settled(Keep::Both).as_deref(),
            Some("top\nmine\ntheirs\nend")
        );
        assert_eq!(resolve_conflict(text, 0, Keep::Ours), None);
        assert_eq!(resolve_conflict(text, 8, Keep::Ours), None);
    }

    #[test]
    fn blame_gives_an_author_and_a_day_per_line() {
        let printed = "abc 1 1 2\nauthor Ada\nauthor-time 1760140800\nsummary first\n\tone\n\
                       abc 2 2\nauthor Ada\nauthor-time 1760140800\nsummary first\n\ttwo\n\
                       000 3 3 1\nauthor Not Committed Yet\nauthor-time 0\nsummary wip\n\tthree\n";
        let lines = parse_blame(printed);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            (lines[1].author.as_str(), lines[1].date.as_str()),
            ("Ada", "2025-10-11")
        );
        assert_eq!(lines[2].date, "1970-01-01");
        assert_eq!(day_of(951_782_400), "2000-02-29");
    }

    #[test]
    fn a_diff_is_laid_side_by_side() {
        let diff = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,4 +1,4 @@\n same\n-old one\n-old two\n\
                    +new one\n end\n+added\n\\ No newline at end of file\n";
        let rows = side_by_side(diff, false);
        let shown: Vec<(Option<usize>, Option<usize>)> = rows
            .iter()
            .map(|row| {
                (
                    row.old.as_ref().map(|it| it.0),
                    row.new.as_ref().map(|it| it.0),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (Some(1), Some(1)),
                (Some(2), Some(2)),
                (Some(3), None),
                (Some(4), Some(3)),
                (None, Some(4)),
            ]
        );
        assert_eq!(
            rows[1].new.as_ref().map(|it| it.1.as_str()),
            Some("new one")
        );
        assert!(side_by_side("", true).is_empty());
        assert_eq!(side_by_side(diff, true)[0].old, Some((0, "x".to_owned())));
    }

    #[test]
    fn the_status_gives_the_branch_and_the_changes() {
        let output = "## main...origin/main [ahead 1]\n M src/app.rs\nA  new file.txt\n\
                      R  old.rs -> src/new.rs\n D gone.md\n?? notes/\nUU both.rs\n";
        let (branch, changes) = parse_status(output);
        assert_eq!(branch.as_deref(), Some("main"));
        let found: Vec<(&str, char)> = changes
            .iter()
            .map(|change| (change.path.as_str(), change.state.letter()))
            .collect();
        assert_eq!(
            found,
            [
                ("src/app.rs", 'M'),
                ("new file.txt", 'A'),
                ("src/new.rs", 'R'),
                ("gone.md", 'D'),
                ("notes/", 'U'),
                ("both.rs", '!'),
            ]
        );
        let (fresh, none) = parse_status("## No commits yet on trunk\n");
        assert_eq!(fresh.as_deref(), Some("trunk"));
        assert!(none.is_empty());
    }

    #[test]
    fn the_diff_gives_the_changed_lines() {
        let output = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3 +3 @@ fn a\n-old\n+new\n\
                      @@ -10,0 +11,2 @@\n+one\n+two\n@@ -20,2 +22,0 @@ tail\n-x\n-y\n";
        assert_eq!(
            parse_diff(output),
            [
                (2, 1, LineChange::Modified),
                (10, 2, LineChange::Added),
                (21, 1, LineChange::Deleted),
            ]
        );
    }
}
