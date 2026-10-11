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

#[cfg(test)]
mod tests {
    use super::*;

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
