//! Three texts merged line by line: what both sides changed alike, what each changed
//! alone, and where they disagree.
//!
//! A line keeps its line end all along, so a merged text has the line ends and the final
//! newline, or lack of one, of the texts it comes from. Two changes are in conflict when
//! they touch the same lines of the base, or add lines at the same place; changes to
//! neighbouring lines are both taken, where git would ask.

use std::collections::HashMap;
use std::ops::Range;

/// How many added and removed lines apart two texts may be before no shared line is
/// looked for between them: the search keeps the square of that distance in memory.
const FARTHEST: usize = 1_500;

/// Lines of the old text and the lines of the new one that replace them, both counted
/// from 0. Either range may be empty: lines added, or lines removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

/// A stretch of a merge. Every line holds its line end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// Lines the three texts share.
    Same(Vec<String>),
    /// Lines we alone changed, from `base` to `ours`.
    Ours {
        base: Vec<String>,
        ours: Vec<String>,
    },
    /// Lines they alone changed, from `base` to `theirs`.
    Theirs {
        base: Vec<String>,
        theirs: Vec<String>,
    },
    /// Lines both sides changed into the same `changed` lines.
    Both {
        base: Vec<String>,
        changed: Vec<String>,
    },
    /// Lines each side changed its own way. `base` is unknown when the conflict was read
    /// from markers that do not tell it.
    Conflict {
        base: Option<Vec<String>>,
        ours: Vec<String>,
        theirs: Vec<String>,
    },
}

/// What a part is, without its lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Same,
    Ours,
    Theirs,
    Both,
    Conflict,
}

/// What to keep of a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
    /// The lines as they were before either side changed them.
    Base,
}

/// A row of the three columns of a merge, the lines without their line ends. A column
/// is empty where its side has fewer lines than another in that part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub kind: Kind,
    /// Which conflict the row belongs to, counted from 0: its place among the choices.
    pub conflict: Option<usize>,
    pub ours: Option<String>,
    /// The line of the merged text; none in a conflict that is not settled yet.
    pub result: Option<String>,
    pub theirs: Option<String>,
}

impl Part {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Same(_) => Kind::Same,
            Self::Ours { .. } => Kind::Ours,
            Self::Theirs { .. } => Kind::Theirs,
            Self::Both { .. } => Kind::Both,
            Self::Conflict { .. } => Kind::Conflict,
        }
    }

    /// The lines our text and their text have in this part.
    fn sides(&self) -> (&[String], &[String]) {
        match self {
            Self::Same(lines) | Self::Both { changed: lines, .. } => (lines, lines),
            Self::Ours { base, ours } => (ours, base),
            Self::Theirs { base, theirs } => (base, theirs),
            Self::Conflict { ours, theirs, .. } => (ours, theirs),
        }
    }

    /// The lines the merged text has in this part; none for a conflict without a choice,
    /// or whose choice is a base nobody knows.
    fn settled(&self, choice: Option<Choice>) -> Option<Vec<&String>> {
        let nothing: &[String] = &[];
        let (first, then): (&[String], &[String]) = match self {
            Self::Same(lines)
            | Self::Ours { ours: lines, .. }
            | Self::Theirs { theirs: lines, .. }
            | Self::Both { changed: lines, .. } => (lines, nothing),
            Self::Conflict { base, ours, theirs } => match choice? {
                Choice::Ours => (ours, nothing),
                Choice::Theirs => (theirs, nothing),
                Choice::OursThenTheirs => (ours, theirs),
                Choice::TheirsThenOurs => (theirs, ours),
                Choice::Base => (base.as_deref()?, nothing),
            },
        };
        Some(first.iter().chain(then).collect())
    }
}

/// The lines of a text, each with its line end.
fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// A number for each line, the same for the same line in every text numbered with `known`:
/// comparing numbers is cheaper than comparing lines.
fn numbered<'a>(lines: &[&'a str], known: &mut HashMap<&'a str, usize>) -> Vec<usize> {
    let number = |line: &&'a str| {
        let next = known.len();
        *known.entry(line).or_insert(next)
    };
    lines.iter().map(number).collect()
}

/// The lines two texts share, as the place of each in the old and in the new text, in
/// order: the longest such list, found the way of Myers. None when the texts are more
/// than `FARTHEST` lines apart.
fn shared(old: &[usize], new: &[usize]) -> Vec<(usize, usize)> {
    let limit = (old.len() + new.len()).min(FARTHEST);
    let middle = limit + 1;
    // How far into `old` each diagonal got. A diagonal is a number of lines removed minus
    // a number of lines added; the one where they are equal is kept at `middle`.
    let mut reached = vec![0_usize; 2 * limit + 3];
    let mut rounds: Vec<Vec<usize>> = Vec::new();
    for round in 0..=limit {
        let mut done = false;
        for at in (middle - round..=middle + round).step_by(2) {
            let added =
                at == middle - round || (at != middle + round && reached[at - 1] < reached[at + 1]);
            let mut old_at = if added {
                reached[at + 1]
            } else {
                reached[at - 1] + 1
            };
            let mut new_at = (old_at + middle).saturating_sub(at);
            while old_at < old.len() && new_at < new.len() && old[old_at] == new[new_at] {
                old_at += 1;
                new_at += 1;
            }
            reached[at] = old_at;
            if old_at >= old.len() && new_at >= new.len() {
                done = true;
                break;
            }
        }
        rounds.push(reached[middle - round..=middle + round].to_vec());
        if done {
            return walked_back(&rounds, old.len(), new.len());
        }
    }
    Vec::new()
}

/// The shared lines met on the way back from the ends of two texts to their starts,
/// through what each round of `shared` reached.
fn walked_back(rounds: &[Vec<usize>], old_len: usize, new_len: usize) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let (mut old_at, mut new_at) = (old_len, new_len);
    for round in (1..rounds.len()).rev() {
        let before = |at: usize| rounds[round - 1].get(at).copied().unwrap_or(0);
        // The diagonal we are on, counted from the lowest one of this round; the round
        // before starts one diagonal higher.
        let Some(at) = (old_at + round).checked_sub(new_at) else {
            break;
        };
        let added = at == 0 || (at < 2 * round && before(at - 2) < before(at));
        let run_start = if added {
            before(at)
        } else {
            before(at - 2) + 1
        };
        let run = old_at.saturating_sub(run_start);
        for _ in 0..run {
            old_at -= 1;
            new_at = new_at.saturating_sub(1);
            pairs.push((old_at, new_at));
        }
        if added {
            new_at = new_at.saturating_sub(1);
        } else {
            old_at = old_at.saturating_sub(1);
        }
    }
    for at in (0..old_at.min(new_at)).rev() {
        pairs.push((at, at));
    }
    pairs.reverse();
    pairs
}

/// What changed between two texts whose lines are numbers.
fn edits(old: &[usize], new: &[usize]) -> Vec<Edit> {
    let same = |(old_line, new_line): &(&usize, &usize)| old_line == new_line;
    let start = old.iter().zip(new).take_while(same).count();
    let (old_rest, new_rest) = (&old[start..], &new[start..]);
    let end = old_rest
        .iter()
        .rev()
        .zip(new_rest.iter().rev())
        .take_while(same)
        .count();
    let old_middle = &old_rest[..old_rest.len() - end];
    let new_middle = &new_rest[..new_rest.len() - end];
    let mut edits = Vec::new();
    let (mut old_at, mut new_at) = (0, 0);
    // The ends of both texts stand for a last shared line, so what follows the last real
    // one is seen as well.
    let ends = (old_middle.len(), new_middle.len());
    for (old_line, new_line) in shared(old_middle, new_middle).into_iter().chain([ends]) {
        if old_line > old_at || new_line > new_at {
            edits.push(Edit {
                old: start + old_at..start + old_line,
                new: start + new_at..start + new_line,
            });
        }
        (old_at, new_at) = (old_line + 1, new_line + 1);
    }
    edits
}

/// What changed from `old` to `new`, line by line, in order. Texts that differ by more
/// than a few thousand lines get one edit from their first to their last difference.
pub fn diff(old: &str, new: &str) -> Vec<Edit> {
    let (old, new) = (lines(old), lines(new));
    let mut known = HashMap::new();
    edits(&numbered(&old, &mut known), &numbered(&new, &mut known))
}

fn owned(lines: &[&str], range: Range<usize>) -> Vec<String> {
    let lines = lines.get(range).unwrap_or_default();
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// Where the lines `edit` replaces end, when they meet `stretch`: they share a line of
/// the base, or both add lines at the same place.
fn end_within(edit: Option<&Edit>, stretch: &Range<usize>) -> Option<usize> {
    let old = &edit?.old;
    (old.start < stretch.end || (old.is_empty() && old == stretch)).then_some(old.end)
}

/// How many lines a side has for `stretch` of the base, given its edits there.
fn length_after(edits: &[Edit], stretch: &Range<usize>) -> usize {
    let added: usize = edits.iter().map(|edit| edit.new.len()).sum();
    let removed: usize = edits.iter().map(|edit| edit.old.len()).sum();
    (stretch.len() + added).saturating_sub(removed)
}

/// `ours` and `theirs`, two texts that both come from `base`, merged line by line: the
/// parts of the merge in order, which together cover the three texts.
pub fn merge(base: &str, ours: &str, theirs: &str) -> Vec<Part> {
    let (base, ours, theirs) = (lines(base), lines(ours), lines(theirs));
    let mut known = HashMap::new();
    let base_numbers = numbered(&base, &mut known);
    let ours_edits = edits(&base_numbers, &numbered(&ours, &mut known));
    let theirs_edits = edits(&base_numbers, &numbered(&theirs, &mut known));
    let mut parts = Vec::new();
    let (mut ours_next, mut theirs_next) = (0, 0);
    let (mut base_at, mut ours_at, mut theirs_at) = (0, 0, 0);
    loop {
        let first = match (ours_edits.get(ours_next), theirs_edits.get(theirs_next)) {
            (None, None) => break,
            (Some(edit), None) | (None, Some(edit)) => edit,
            // Lines added before a line come before a change to that line.
            (Some(ours), Some(theirs)) => {
                if (theirs.old.start, theirs.old.end) < (ours.old.start, ours.old.end) {
                    theirs
                } else {
                    ours
                }
            }
        };
        // The lines of the base this part is about: those of the first edit, then those
        // of every edit that meets them.
        let mut stretch = first.old.clone();
        let (ours_from, theirs_from) = (ours_next, theirs_next);
        loop {
            if let Some(end) = end_within(ours_edits.get(ours_next), &stretch) {
                stretch.end = stretch.end.max(end);
                ours_next += 1;
            } else if let Some(end) = end_within(theirs_edits.get(theirs_next), &stretch) {
                stretch.end = stretch.end.max(end);
                theirs_next += 1;
            } else {
                break;
            }
        }
        let untouched = stretch.start.saturating_sub(base_at);
        if untouched > 0 {
            parts.push(Part::Same(owned(&base, base_at..stretch.start)));
        }
        ours_at += untouched;
        theirs_at += untouched;
        let ours_here = &ours_edits[ours_from..ours_next];
        let theirs_here = &theirs_edits[theirs_from..theirs_next];
        let ours_end = ours_at + length_after(ours_here, &stretch);
        let theirs_end = theirs_at + length_after(theirs_here, &stretch);
        let was = owned(&base, stretch.clone());
        let ours_lines = owned(&ours, ours_at..ours_end);
        let theirs_lines = owned(&theirs, theirs_at..theirs_end);
        parts.push(if theirs_here.is_empty() {
            Part::Ours {
                base: was,
                ours: ours_lines,
            }
        } else if ours_here.is_empty() {
            Part::Theirs {
                base: was,
                theirs: theirs_lines,
            }
        } else if ours_lines == theirs_lines {
            Part::Both {
                base: was,
                changed: ours_lines,
            }
        } else {
            Part::Conflict {
                base: Some(was),
                ours: ours_lines,
                theirs: theirs_lines,
            }
        });
        (base_at, ours_at, theirs_at) = (stretch.end, ours_end, theirs_end);
    }
    if base_at < base.len() {
        parts.push(Part::Same(owned(&base, base_at..base.len())));
    }
    parts
}

/// Each part with the number of the conflict it is, counted from 0, when it is one.
fn with_conflicts(parts: &[Part]) -> impl Iterator<Item = (&Part, Option<usize>)> {
    let mut conflicts = 0;
    parts.iter().map(move |part| {
        let conflict = (part.kind() == Kind::Conflict).then(|| {
            conflicts += 1;
            conflicts - 1
        });
        (part, conflict)
    })
}

/// The choice made for a conflict, when one was.
fn choice_for(choices: &[Option<Choice>], conflict: Option<usize>) -> Option<Choice> {
    choices.get(conflict?).copied().flatten()
}

/// The line end the lines of a merge use: that of its first line.
fn line_end(parts: &[Part]) -> &'static str {
    let first = parts.iter().find_map(|part| {
        let (ours, theirs) = part.sides();
        ours.first().or(theirs.first())
    });
    if first.is_some_and(|line| line.ends_with("\r\n")) {
        "\r\n"
    } else {
        "\n"
    }
}

/// Adds a line to a text, on a line of its own even when the last line of the text has
/// no line end.
fn add_line(text: &mut String, line: &str, line_end: &str) {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push_str(line_end);
    }
    text.push_str(line);
}

/// The merged text: what each side changed alone and what both changed alike are taken,
/// and each conflict is settled by the choice at its place in `choices`, conflicts being
/// counted from 0. A conflict without a choice is written between markers, the way git
/// writes it, as is one settled for a base that is unknown.
pub fn resolved(parts: &[Part], choices: &[Option<Choice>]) -> String {
    let line_end = line_end(parts);
    let mut text = String::new();
    for (part, conflict) in with_conflicts(parts) {
        if let Some(kept) = part.settled(choice_for(choices, conflict)) {
            for line in kept {
                add_line(&mut text, line, line_end);
            }
            continue;
        }
        let Part::Conflict { base, ours, theirs } = part else {
            continue;
        };
        let nothing = Vec::new();
        let sections = [
            (Some("<<<<<<< ours"), Some(ours)),
            (base.as_ref().map(|_| "||||||| base"), base.as_ref()),
            (Some("======="), Some(theirs)),
            (Some(">>>>>>> theirs"), Some(&nothing)),
        ];
        for (marker, section) in sections {
            if let Some(marker) = marker {
                add_line(&mut text, &format!("{marker}{line_end}"), line_end);
            }
            for line in section.into_iter().flatten() {
                add_line(&mut text, line, line_end);
            }
        }
    }
    text
}

/// How many conflicts `resolved` would still write between markers.
pub fn unresolved(parts: &[Part], choices: &[Option<Choice>]) -> usize {
    let open = |(part, conflict): &(&Part, Option<usize>)| {
        part.settled(choice_for(choices, *conflict)).is_none()
    };
    with_conflicts(parts).filter(open).count()
}

/// The parts as rows of three columns that stay level: ours on the left, the merged text
/// in the middle, theirs on the right. Each part takes as many rows as its longest
/// column, the shorter ones being left empty below their last line.
pub fn rows(parts: &[Part], choices: &[Option<Choice>]) -> Vec<Row> {
    let shown = |lines: &[&String], at: usize| {
        let line = lines.get(at)?.as_str();
        let line = line.strip_suffix('\n').unwrap_or(line);
        Some(line.strip_suffix('\r').unwrap_or(line).to_owned())
    };
    let mut rows = Vec::new();
    for (part, conflict) in with_conflicts(parts) {
        let (ours, theirs) = part.sides();
        let (ours, theirs): (Vec<&String>, Vec<&String>) =
            (ours.iter().collect(), theirs.iter().collect());
        let result = part
            .settled(choice_for(choices, conflict))
            .unwrap_or_default();
        for at in 0..ours.len().max(theirs.len()).max(result.len()) {
            rows.push(Row {
                kind: part.kind(),
                conflict,
                ours: shown(&ours, at),
                result: shown(&result, at),
                theirs: shown(&theirs, at),
            });
        }
    }
    rows
}

/// The parts of a text that holds conflicts between markers, as git leaves it in the
/// working file: what stands outside the markers is shared, and each conflict has our
/// lines, their lines, and the base when a `|||||||` section tells it. `None` when the
/// text holds no whole conflict.
pub fn from_markers(text: &str) -> Option<Vec<Part>> {
    let lines = lines(text);
    // A marker is seven times its sign, alone on its line or followed by a name.
    let marks = |at: usize, mark: &str| {
        let rest = lines.get(at).and_then(|line| line.strip_prefix(mark));
        rest.is_some_and(|rest| rest.trim_end().is_empty() || rest.starts_with(' '))
    };
    // Where their lines start and where they end, for a conflict that opens at `start`.
    let closing = |start: usize| {
        let end = (start + 1..lines.len()).find(|&at| marks(at, ">>>>>>>"))?;
        let middle = (start + 1..end).find(|&at| marks(at, "======="))?;
        let reopened = (start + 1..middle).any(|at| marks(at, "<<<<<<<"));
        (!reopened).then_some((middle, end))
    };
    let mut parts = Vec::new();
    let (mut at, mut shared_from) = (0, 0);
    while at < lines.len() {
        let Some((middle, end)) = marks(at, "<<<<<<<").then(|| closing(at)).flatten() else {
            at += 1;
            continue;
        };
        if at > shared_from {
            parts.push(Part::Same(owned(&lines, shared_from..at)));
        }
        let base_mark = (at + 1..middle).find(|&line| marks(line, "|||||||"));
        parts.push(Part::Conflict {
            base: base_mark.map(|mark| owned(&lines, mark + 1..middle)),
            ours: owned(&lines, at + 1..base_mark.unwrap_or(middle)),
            theirs: owned(&lines, middle + 1..end),
        });
        at = end + 1;
        shared_from = at;
    }
    if parts.is_empty() {
        return None;
    }
    if shared_from < lines.len() {
        parts.push(Part::Same(owned(&lines, shared_from..lines.len())));
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(written: &str) -> Vec<String> {
        lines(written).into_iter().map(str::to_owned).collect()
    }

    fn same(written: &str) -> Part {
        Part::Same(text(written))
    }

    fn conflict(base: Option<&str>, ours: &str, theirs: &str) -> Part {
        Part::Conflict {
            base: base.map(text),
            ours: text(ours),
            theirs: text(theirs),
        }
    }

    /// The merged text with every conflict left between markers.
    fn merged(base: &str, ours: &str, theirs: &str) -> String {
        resolved(&merge(base, ours, theirs), &[])
    }

    fn edit(old: Range<usize>, new: Range<usize>) -> Edit {
        Edit { old, new }
    }

    /// The length of the longest list of lines two texts share, the slow and sure way.
    fn longest_shared(old: &[usize], new: &[usize]) -> usize {
        let mut lengths = vec![vec![0_usize; new.len() + 1]; old.len() + 1];
        for (old_at, old_line) in old.iter().enumerate() {
            for (new_at, new_line) in new.iter().enumerate() {
                lengths[old_at + 1][new_at + 1] = if old_line == new_line {
                    lengths[old_at][new_at] + 1
                } else {
                    lengths[old_at][new_at + 1].max(lengths[old_at + 1][new_at])
                };
            }
        }
        lengths[old.len()][new.len()]
    }

    #[test]
    fn a_diff_tells_the_lines_replaced_added_and_removed() {
        assert_eq!(diff("a\nb\nc\n", "a\nb\nc\n"), []);
        assert_eq!(diff("", ""), []);
        assert_eq!(diff("a\nb\nc\n", "a\nx\nc\n"), [edit(1..2, 1..2)]);
        assert_eq!(diff("a\nc\n", "a\nb\nc\n"), [edit(1..1, 1..2)]);
        assert_eq!(diff("a\nb\nc\n", "a\nc\n"), [edit(1..2, 1..1)]);
        assert_eq!(diff("", "a\nb\n"), [edit(0..0, 0..2)]);
        assert_eq!(diff("a\nb\n", ""), [edit(0..2, 0..0)]);
        assert_eq!(
            diff("a\nb\nc\nd\ne\n", "x\nb\nc\nd\ny\nz\n"),
            [edit(0..1, 0..1), edit(4..5, 4..6)]
        );
        // The same line with and without a line end is not the same line.
        assert_eq!(diff("a\nb", "a\nb\n"), [edit(1..2, 1..2)]);
    }

    #[test]
    fn a_diff_keeps_as_many_lines_as_two_texts_can_share() {
        let mut seed = 7_u64;
        let mut random = |below: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from((seed >> 33) % below).unwrap_or(0)
        };
        for _ in 0..400 {
            let old: Vec<usize> = (0..random(14)).map(|_| random(4)).collect();
            let new: Vec<usize> = (0..random(14)).map(|_| random(4)).collect();
            let edits = edits(&old, &new);
            let removed: usize = edits.iter().map(|edit| edit.old.len()).sum();
            assert_eq!(
                old.len() - removed,
                longest_shared(&old, &new),
                "{old:?} {new:?}"
            );
            // The old text with its edits applied is the new one.
            let mut rebuilt: Vec<usize> = Vec::new();
            let mut old_at = 0;
            for edit in &edits {
                rebuilt.extend(&old[old_at..edit.old.start]);
                assert_eq!(rebuilt.len(), edit.new.start, "{old:?} {new:?}");
                rebuilt.extend(&new[edit.new.clone()]);
                old_at = edit.old.end;
            }
            rebuilt.extend(&old[old_at..]);
            assert_eq!(rebuilt, new, "{old:?}");
        }
    }

    #[test]
    fn a_large_text_with_few_changes_is_compared_line_by_line() {
        let old: String = (0..20_000).map(|line| format!("line {line}\n")).collect();
        let new = old
            .replace("line 5\n", "five\n")
            .replace("line 19990\n", "");
        assert_eq!(
            diff(&old, &new),
            [edit(5..6, 5..6), edit(19_990..19_991, 19_990..19_990)]
        );
    }

    #[test]
    fn texts_too_far_apart_are_one_change_from_first_to_last_difference() {
        let old: String = (0..2_000).map(|line| format!("old {line}\n")).collect();
        let new: String = (0..2_000).map(|line| format!("new {line}\n")).collect();
        let (old, new) = (
            format!("top\n{old}middle\n{old}end\n"),
            format!("top\n{new}middle\n{new}end\n"),
        );
        // The line in the middle, which they share, is not looked for.
        assert_eq!(diff(&old, &new), [edit(1..4_002, 1..4_002)]);
        let parts = merge(&old, &new, &old);
        assert_eq!(unresolved(&parts, &[]), 0);
        assert_eq!(resolved(&parts, &[]), new);
    }

    #[test]
    fn identical_texts_are_one_shared_part() {
        let written = "one\ntwo\nthree\n";
        assert_eq!(merge(written, written, written), [same(written)]);
        assert_eq!(merged(written, written, written), written);
    }

    #[test]
    fn empty_texts_have_no_part() {
        assert_eq!(merge("", "", ""), []);
        assert_eq!(merged("", "", ""), "");
        assert_eq!(rows(&[], &[]), []);
        assert_eq!(unresolved(&[], &[]), 0);
    }

    #[test]
    fn a_change_made_on_one_side_alone_is_taken() {
        let (base, changed) = ("one\ntwo\nthree\n", "one\n2\nthree\n");
        let ours = Part::Ours {
            base: text("two\n"),
            ours: text("2\n"),
        };
        assert_eq!(
            merge(base, changed, base),
            [same("one\n"), ours, same("three\n")]
        );
        assert_eq!(merged(base, changed, base), changed);
        let theirs = Part::Theirs {
            base: text("two\n"),
            theirs: text("2\n"),
        };
        assert_eq!(
            merge(base, base, changed),
            [same("one\n"), theirs, same("three\n")]
        );
        assert_eq!(merged(base, base, changed), changed);
    }

    #[test]
    fn the_same_change_on_both_sides_is_taken_once() {
        // What `git merge-file -p --diff3` prints for these: "a\nX\nc\n".
        let parts = merge("a\nb\nc\n", "a\nX\nc\n", "a\nX\nc\n");
        let both = Part::Both {
            base: text("b\n"),
            changed: text("X\n"),
        };
        assert_eq!(parts, [same("a\n"), both, same("c\n")]);
        assert_eq!(resolved(&parts, &[]), "a\nX\nc\n");
        assert_eq!(unresolved(&parts, &[]), 0);
    }

    #[test]
    fn changes_far_apart_are_both_taken() {
        // Checked against `git merge-file -p --diff3`.
        let base = "a\nb\nc\nd\ne\nf\ng\n";
        let merged = merged(base, "A\nb\nc\nd\ne\nf\ng\n", "a\nb\nc\nd\ne\nf\nG\n");
        assert_eq!(merged, "A\nb\nc\nd\ne\nf\nG\n");
    }

    #[test]
    fn changes_to_neighbouring_lines_are_both_taken() {
        // Git asks here; each side changed a line the other left alone, so we do not.
        let parts = merge("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n");
        let ours = Part::Ours {
            base: text("b\n"),
            ours: text("B\n"),
        };
        let theirs = Part::Theirs {
            base: text("c\n"),
            theirs: text("C\n"),
        };
        assert_eq!(parts, [same("a\n"), ours, theirs, same("d\n")]);
        assert_eq!(resolved(&parts, &[]), "a\nB\nC\nd\n");
    }

    #[test]
    fn lines_added_before_a_line_the_other_side_changed_are_both_taken() {
        let merged = merged("a\nb\n", "a\nnew\nb\n", "a\nB\n");
        assert_eq!(merged, "a\nnew\nB\n");
    }

    #[test]
    fn changes_to_the_same_line_are_a_conflict_with_its_three_versions() {
        let base = "one\ntwo\nthree\nfour\n";
        let ours = "one\nTWO\nthree\nfour\n";
        let theirs = "one\ndeux\nthree\nfour\n";
        let parts = merge(base, ours, theirs);
        let expected = [
            same("one\n"),
            conflict(Some("two\n"), "TWO\n", "deux\n"),
            same("three\nfour\n"),
        ];
        assert_eq!(parts, expected);
        assert_eq!(unresolved(&parts, &[]), 1);
        // What `git merge-file -p --diff3 -L ours -L base -L theirs` prints.
        let printed = "one\n<<<<<<< ours\nTWO\n||||||| base\ntwo\n=======\ndeux\n\
                       >>>>>>> theirs\nthree\nfour\n";
        assert_eq!(resolved(&parts, &[]), printed);
    }

    #[test]
    fn changes_that_overlap_in_part_are_one_conflict_over_all_their_lines() {
        let parts = merge("a\nb\nc\nd\ne\n", "a\nB\nC\nd\ne\n", "a\nb\nc2\nD\ne\n");
        let expected = [
            same("a\n"),
            conflict(Some("b\nc\nd\n"), "B\nC\nd\n", "b\nc2\nD\n"),
            same("e\n"),
        ];
        assert_eq!(parts, expected);
    }

    #[test]
    fn lines_added_at_the_very_start_and_the_very_end_are_taken() {
        // Checked against `git merge-file -p --diff3`.
        let parts = merge("m\n", "top\nm\n", "m\nbottom\n");
        let ours = Part::Ours {
            base: Vec::new(),
            ours: text("top\n"),
        };
        let theirs = Part::Theirs {
            base: Vec::new(),
            theirs: text("bottom\n"),
        };
        assert_eq!(parts, [ours, same("m\n"), theirs]);
        assert_eq!(resolved(&parts, &[]), "top\nm\nbottom\n");
    }

    #[test]
    fn different_lines_added_at_the_same_place_are_a_conflict() {
        let parts = merge("a\nb\n", "a\nmine\nb\n", "a\nyours\nb\n");
        let expected = [
            same("a\n"),
            conflict(Some(""), "mine\n", "yours\n"),
            same("b\n"),
        ];
        assert_eq!(parts, expected);
    }

    #[test]
    fn a_line_removed_on_one_side_and_changed_on_the_other_is_a_conflict() {
        let parts = merge("one\ntwo\nthree\n", "one\nthree\n", "one\n2\nthree\n");
        let expected = [
            same("one\n"),
            conflict(Some("two\n"), "", "2\n"),
            same("three\n"),
        ];
        assert_eq!(parts, expected);
        // What `git merge-file -p --diff3 -L ours -L base -L theirs` prints.
        let printed = "one\n<<<<<<< ours\n||||||| base\ntwo\n=======\n2\n>>>>>>> theirs\nthree\n";
        assert_eq!(resolved(&parts, &[]), printed);
        assert_eq!(resolved(&parts, &[Some(Choice::Ours)]), "one\nthree\n");
    }

    #[test]
    fn two_texts_written_from_nothing_are_a_conflict() {
        let parts = merge("", "mine\n", "yours\n");
        assert_eq!(parts, [conflict(Some(""), "mine\n", "yours\n")]);
        // What `git merge-file -p --diff3 -L ours -L base -L theirs` prints.
        let printed = "<<<<<<< ours\nmine\n||||||| base\n=======\nyours\n>>>>>>> theirs\n";
        assert_eq!(resolved(&parts, &[]), printed);
        assert_eq!(merged("", "new\n", ""), "new\n");
        assert_eq!(merged("", "new\n", "new\n"), "new\n");
        assert_eq!(merged("old\n", "", "old\n"), "");
    }

    #[test]
    fn several_conflicts_and_a_change_between_them_match_what_git_prints() {
        let base = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let ours = "a\nB1\nc\nd\ne\nF1\nF2\ng\nh\n";
        let theirs = "a\nB2\nc\nD\ne\ng\nh\n";
        // What `git merge-file -p --diff3 -L ours -L base -L theirs` prints.
        let printed = "a\n<<<<<<< ours\nB1\n||||||| base\nb\n=======\nB2\n>>>>>>> theirs\n\
                       c\nD\ne\n<<<<<<< ours\nF1\nF2\n||||||| base\nf\n=======\n\
                       >>>>>>> theirs\ng\nh\n";
        let parts = merge(base, ours, theirs);
        assert_eq!(resolved(&parts, &[]), printed);
        assert_eq!(unresolved(&parts, &[]), 2);
    }

    #[test]
    fn each_choice_writes_its_lines_in_place_of_the_conflict() {
        let parts = merge(
            "one\ntwo\nthree\n",
            "one\nTWO\nthree\n",
            "one\ndeux\nthree\n",
        );
        let chosen = |choice| resolved(&parts, &[Some(choice)]);
        assert_eq!(chosen(Choice::Ours), "one\nTWO\nthree\n");
        assert_eq!(chosen(Choice::Theirs), "one\ndeux\nthree\n");
        assert_eq!(chosen(Choice::OursThenTheirs), "one\nTWO\ndeux\nthree\n");
        assert_eq!(chosen(Choice::TheirsThenOurs), "one\ndeux\nTWO\nthree\n");
        assert_eq!(chosen(Choice::Base), "one\ntwo\nthree\n");
    }

    #[test]
    fn conflicts_are_counted_until_each_has_a_choice() {
        let base = "a\nb\nc\nd\ne\n";
        let parts = merge(base, "a\nB1\nc\nD1\ne\n", "a\nB2\nc\nD2\ne\n");
        assert_eq!(unresolved(&parts, &[]), 2);
        assert_eq!(unresolved(&parts, &[None, Some(Choice::Theirs)]), 1);
        let second_only = resolved(&parts, &[None, Some(Choice::Theirs)]);
        assert_eq!(
            second_only,
            "a\n<<<<<<< ours\nB1\n||||||| base\nb\n=======\nB2\n>>>>>>> theirs\nc\nD2\ne\n"
        );
        let choices = [Some(Choice::Ours), Some(Choice::Theirs)];
        assert_eq!(unresolved(&parts, &choices), 0);
        assert_eq!(resolved(&parts, &choices), "a\nB1\nc\nD2\ne\n");
    }

    #[test]
    fn line_ends_and_the_final_newline_come_through_a_merge() {
        let merged_crlf = merged("a\r\nb\r\nc\r\n", "A\r\nb\r\nc\r\n", "a\r\nb\r\nC\r\n");
        assert_eq!(merged_crlf, "A\r\nb\r\nC\r\n");
        assert_eq!(merged("a\nb\nc", "A\nb\nc", "a\nb\nc"), "A\nb\nc");
        assert_eq!(merged("a\nb\nc", "a\nb\nc", "a\nb\nC"), "a\nb\nC");
        // They gave the last line its line end, we changed the first line.
        assert_eq!(merged("a\nb\nc", "A\nb\nc", "a\nb\nc\n"), "A\nb\nc\n");
        assert_eq!(merged("a\r\nb", "a\r\nb", "a\r\nb"), "a\r\nb");
    }

    #[test]
    fn a_conflict_takes_the_line_ends_of_its_text() {
        // Both checked against `git merge-file -p --diff3 -L ours -L base -L theirs`.
        let crlf = merged(
            "one\r\ntwo\r\nthree\r\n",
            "one\r\nTWO\r\nthree\r\n",
            "one\r\ndeux\r\nthree\r\n",
        );
        assert_eq!(
            crlf,
            "one\r\n<<<<<<< ours\r\nTWO\r\n||||||| base\r\ntwo\r\n=======\r\ndeux\r\n\
             >>>>>>> theirs\r\nthree\r\n"
        );
        let parts = merge("one\ntwo", "one\nTWO", "one\ndeux");
        assert_eq!(
            resolved(&parts, &[]),
            "one\n<<<<<<< ours\nTWO\n||||||| base\ntwo\n=======\ndeux\n>>>>>>> theirs\n"
        );
        // Two last lines without a line end do not run into each other.
        assert_eq!(
            resolved(&parts, &[Some(Choice::OursThenTheirs)]),
            "one\nTWO\ndeux"
        );
    }

    #[test]
    fn rows_keep_the_three_columns_level() {
        let parts = merge("a\nb\nc\nd\ne\n", "a\nB1\nB2\nc\nd\ne\n", "a\nb\nc\ne\n");
        let row = |kind, ours: Option<&str>, result: Option<&str>, theirs: Option<&str>| Row {
            kind,
            conflict: None,
            ours: ours.map(str::to_owned),
            result: result.map(str::to_owned),
            theirs: theirs.map(str::to_owned),
        };
        let expected = [
            row(Kind::Same, Some("a"), Some("a"), Some("a")),
            row(Kind::Ours, Some("B1"), Some("B1"), Some("b")),
            row(Kind::Ours, Some("B2"), Some("B2"), None),
            row(Kind::Same, Some("c"), Some("c"), Some("c")),
            row(Kind::Theirs, Some("d"), None, None),
            row(Kind::Same, Some("e"), Some("e"), Some("e")),
        ];
        assert_eq!(rows(&parts, &[]), expected);
    }

    #[test]
    fn the_rows_of_a_conflict_show_its_choice_in_the_middle() {
        let parts = merge("a\nb\nz\n", "a\nmine 1\nmine 2\nz\n", "a\nyours\nz\n");
        let middle = |choices: &[Option<Choice>]| -> Vec<(Option<usize>, Option<String>)> {
            let rows = rows(&parts, choices);
            rows.into_iter()
                .map(|row| (row.conflict, row.result))
                .collect()
        };
        let line = |conflict, written: &str| (conflict, Some(written.to_owned()));
        assert_eq!(
            middle(&[]),
            [
                line(None, "a"),
                (Some(0), None),
                (Some(0), None),
                line(None, "z")
            ]
        );
        assert_eq!(
            middle(&[Some(Choice::Theirs)]),
            [
                line(None, "a"),
                line(Some(0), "yours"),
                (Some(0), None),
                line(None, "z")
            ]
        );
        let both = rows(&parts, &[Some(Choice::TheirsThenOurs)]);
        assert_eq!(both.len(), 5);
        assert_eq!(both[3].kind, Kind::Conflict);
        assert_eq!(both[3].ours, None);
        assert_eq!(both[3].result.as_deref(), Some("mine 2"));
        assert_eq!(both[3].theirs, None);
    }

    #[test]
    fn rows_lose_the_line_ends_and_number_each_conflict() {
        let base = "a\r\nb\r\nc\r\nd";
        let parts = merge(base, "a\r\nB1\r\nc\r\nD1", "a\r\nB2\r\nc\r\nD2");
        let rows = rows(&parts, &[]);
        let ours: Vec<_> = rows.iter().map(|row| row.ours.as_deref()).collect();
        assert_eq!(ours, [Some("a"), Some("B1"), Some("c"), Some("D1")]);
        let conflicts: Vec<_> = rows.iter().map(|row| row.conflict).collect();
        assert_eq!(conflicts, [None, Some(0), None, Some(1)]);
    }

    #[test]
    fn markers_with_a_base_section_are_read_back_into_the_parts_of_the_merge() {
        let base = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let ours = "a\nB1\nc\nd\ne\nF1\nF2\ng\nh\n";
        let theirs = "a\nB2\nc\nD\ne\ng\nh\n";
        let printed = merged(base, ours, theirs);
        let expected = [
            same("a\n"),
            conflict(Some("b\n"), "B1\n", "B2\n"),
            same("c\nD\ne\n"),
            conflict(Some("f\n"), "F1\nF2\n", ""),
            same("g\nh\n"),
        ];
        assert_eq!(from_markers(&printed).as_deref(), Some(&expected[..]));
    }

    #[test]
    fn markers_without_a_base_section_leave_the_base_unknown() {
        let written = "<<<<<<< HEAD\nmine\n=======\nyours\nmore\n>>>>>>> feature/x\nend";
        let parts = from_markers(written).unwrap_or_default();
        assert_eq!(
            parts,
            [conflict(None, "mine\n", "yours\nmore\n"), same("end")]
        );
        // A base nobody knows cannot be chosen: the conflict stays.
        assert_eq!(unresolved(&parts, &[Some(Choice::Base)]), 1);
        assert_eq!(
            resolved(&parts, &[Some(Choice::Base)]),
            "<<<<<<< ours\nmine\n=======\nyours\nmore\n>>>>>>> theirs\nend"
        );
        assert_eq!(rows(&parts, &[Some(Choice::Base)])[0].result, None);
        assert_eq!(
            resolved(&parts, &[Some(Choice::Theirs)]),
            "yours\nmore\nend"
        );
    }

    #[test]
    fn a_text_without_a_whole_conflict_has_no_markers_to_read() {
        assert_eq!(from_markers(""), None);
        assert_eq!(from_markers("a\nb\n"), None);
        assert_eq!(from_markers("<<<<<<< HEAD\nmine\n=======\nyours\n"), None);
        assert_eq!(from_markers("<<<<<<< HEAD\nmine\n>>>>>>> other\n"), None);
        // A title underlined in Markdown, and signs that are not seven.
        assert_eq!(
            from_markers("<<<<<<<<\nTitle\n==========\n>>>>>>>>\n"),
            None
        );
    }

    #[test]
    fn a_conflict_left_open_before_a_whole_one_is_plain_text() {
        let written = "<<<<<<< HEAD\nlost\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> other\n";
        let expected = [
            same("<<<<<<< HEAD\nlost\n"),
            conflict(None, "mine\n", "yours\n"),
        ];
        assert_eq!(from_markers(written).as_deref(), Some(&expected[..]));
    }

    #[test]
    fn markers_with_windows_line_ends_are_read() {
        let written = "a\r\n<<<<<<< ours\r\nmine\r\n||||||| base\r\n=======\r\nyours\r\n\
                       >>>>>>> theirs\r\n";
        let parts = from_markers(written).unwrap_or_default();
        assert_eq!(
            parts,
            [same("a\r\n"), conflict(Some(""), "mine\r\n", "yours\r\n")]
        );
        assert_eq!(resolved(&parts, &[]), written);
    }

    #[test]
    fn a_text_with_markers_goes_to_parts_and_back() {
        // What `git merge-file -p --diff3 -L ours -L base -L theirs` printed.
        let printed = "a\n<<<<<<< ours\nB1\n||||||| base\nb\n=======\nB2\n>>>>>>> theirs\n\
                       c\nD\ne\n<<<<<<< ours\nF1\nF2\n||||||| base\nf\n=======\n\
                       >>>>>>> theirs\ng\nh\n";
        let parts = from_markers(printed).unwrap_or_default();
        assert_eq!(unresolved(&parts, &[]), 2);
        assert_eq!(resolved(&parts, &[]), printed);
        let choices = [Some(Choice::Theirs), Some(Choice::Ours)];
        assert_eq!(resolved(&parts, &choices), "a\nB2\nc\nD\ne\nF1\nF2\ng\nh\n");
        let choices = [Some(Choice::Base), Some(Choice::TheirsThenOurs)];
        assert_eq!(resolved(&parts, &choices), "a\nb\nc\nD\ne\nF1\nF2\ng\nh\n");
        assert_eq!(unresolved(&parts, &choices), 0);
    }
}
