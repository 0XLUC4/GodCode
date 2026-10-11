//! Which lines the tests ran: coverage reports read into lines and how often each ran.
//!
//! Two formats are read. LCOV tracefiles, written by `cargo llvm-cov --lcov`,
//! `pytest --cov --cov-report=lcov`, `vitest --coverage` and `c8`; and Go's coverprofile,
//! written by `go test -coverprofile`.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::Path;

/// A Go block longer than this is taken for a malformed line and skipped.
const LONGEST_BLOCK: usize = 100_000;

/// The lines of one file that the tests could run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCoverage {
    /// As written in the report.
    pub path: String,
    /// Each line, counted from 0, with how many times it ran; sorted by line.
    pub lines: Vec<(usize, u64)>,
}

impl FileCoverage {
    /// How many of the known lines ran, out of 100. A file with no known line is at 0.
    pub fn percent(&self) -> f32 {
        percent_of(self.lines.iter())
    }
}

/// How many of the known lines of all the files ran, out of 100.
pub fn total_percent(all: &[FileCoverage]) -> f32 {
    percent_of(all.iter().flat_map(|file| &file.lines))
}

fn percent_of<'a>(lines: impl Iterator<Item = &'a (usize, u64)>) -> f32 {
    let (ran, known) = lines.fold((0usize, 0usize), |(ran, known), (_, hits)| {
        (ran + usize::from(*hits > 0), known + 1)
    });
    if known == 0 {
        return 0.0;
    }
    ran as f32 / known as f32 * 100.0
}

/// The format a report is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Lcov,
    GoProfile,
}

/// The format of a report, told from its first lines; `None` when it is neither.
pub fn format(report: &str) -> Option<Format> {
    let mut lines = report
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    if lines.clone().next()?.starts_with("mode:") {
        return Some(Format::GoProfile);
    }
    let lcov =
        |line: &str| line.starts_with("SF:") || line.starts_with("TN:") || line == "end_of_record";
    lines.any(lcov).then_some(Format::Lcov)
}

/// A report in either format; nothing when it is in neither.
pub fn read(report: &str) -> Vec<FileCoverage> {
    match format(report) {
        Some(Format::Lcov) => lcov(report),
        Some(Format::GoProfile) => go_profile(report),
        None => Vec::new(),
    }
}

/// The files of a report being read, in the order the report first names them.
#[derive(Default)]
struct Files(Vec<(String, BTreeMap<usize, u64>)>);

impl Files {
    /// Where the file written `path` stands, added when the report had not named it yet.
    fn at(&mut self, path: &str) -> usize {
        let known = self.0.iter().position(|(written, _)| written == path);
        known.unwrap_or_else(|| {
            self.0.push((path.to_owned(), BTreeMap::new()));
            self.0.len() - 1
        })
    }

    fn read(self) -> Vec<FileCoverage> {
        let file = |(path, lines): (String, BTreeMap<usize, u64>)| FileCoverage {
            path,
            lines: lines.into_iter().collect(),
        };
        self.0.into_iter().map(file).collect()
    }
}

/// An LCOV tracefile. A file written in several records has the hits of each added up.
pub fn lcov(report: &str) -> Vec<FileCoverage> {
    let mut files = Files::default();
    let mut current = None;
    for line in report.lines().map(str::trim) {
        if let Some(path) = line.strip_prefix("SF:") {
            let path = path.trim();
            current = (!path.is_empty()).then(|| files.at(path));
        } else if line == "end_of_record" {
            current = None;
        } else if let (Some(at), Some(ran)) = (current, line.strip_prefix("DA:")) {
            // `DA:<line>,<hits>`, a checksum sometimes following.
            let mut fields = ran.split(',').map(str::trim);
            let number = fields.next().and_then(|field| field.parse::<usize>().ok());
            let hits = fields.next().and_then(|field| field.parse::<u64>().ok());
            let (Some(number), Some(hits)) = (number, hits) else {
                continue;
            };
            // Lines are counted from 1 in the report.
            let Some(number) = number.checked_sub(1) else {
                continue;
            };
            let known = files.0[at].1.entry(number).or_default();
            *known = known.saturating_add(hits);
        }
    }
    files.read()
}

/// The first and last line, counted from 0, of a Go block: `3.24,5.2`.
fn block(written: &str) -> Option<(usize, usize)> {
    let (start, end) = written.split_once(',')?;
    let line = |position: &str| {
        position
            .split_once('.')?
            .0
            .parse::<usize>()
            .ok()?
            .checked_sub(1)
    };
    let (start, end) = (line(start)?, line(end)?);
    (start <= end && end - start < LONGEST_BLOCK).then_some((start, end))
}

/// A Go coverprofile, in any of its modes. Every line of a block ran as often as the
/// block; a line that several blocks cover ran as often as the one that ran the most.
pub fn go_profile(profile: &str) -> Vec<FileCoverage> {
    let mut files = Files::default();
    for line in profile.lines() {
        // `file:startLine.startCol,endLine.endCol numStatements count`
        let mut fields = line.trim().rsplitn(3, char::is_whitespace);
        let count = fields.next().and_then(|field| field.parse::<u64>().ok());
        let place = fields
            .nth(1)
            .and_then(|place| place.trim().rsplit_once(':'));
        let (Some(count), Some((path, written))) = (count, place) else {
            continue;
        };
        let Some((start, end)) = block(written) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let at = files.at(path);
        for number in start..=end {
            let known = files.0[at].1.entry(number).or_default();
            *known = (*known).max(count);
        }
    }
    files.read()
}

/// The folders and the name a path is made of, written with `/` or `\`. The `?` of a
/// Windows `\\?\` path is left out.
fn parts(path: &str) -> Vec<&str> {
    let named = |part: &&str| !part.is_empty() && !matches!(*part, "." | "?");
    path.split(['/', '\\']).filter(named).collect()
}

/// The report of a file of the project under `root`, however the report wrote its path:
/// whole, or from the root, with `/` or `\`, in either case of letters.
///
/// Go writes `module/folder/file.go`, the module being no folder of the project. A Go file
/// is given the report whose path ends like its own for the longest: all of its path from
/// the root, or at least its folder and its name when its module is kept in a folder of
/// the project.
pub fn of_file<'a>(all: &'a [FileCoverage], path: &Path, root: &Path) -> Option<&'a FileCoverage> {
    let whole = root.join(path);
    let (whole, root) = (whole.to_string_lossy(), root.to_string_lossy());
    let (wanted, root) = (parts(&whole), parts(&root));
    let same = |(one, other): &(&&str, &&str)| one.eq_ignore_ascii_case(other);
    let under = wanted.len() >= root.len() && root.iter().zip(&wanted).all(|pair| same(&pair));
    let inside = wanted.len() - if under { root.len() } else { 0 };
    let go = wanted.last().is_some_and(|name| name.ends_with(".go"));
    let mut best: Option<((usize, Reverse<usize>), &FileCoverage)> = None;
    for file in all {
        let written = parts(&file.path);
        let ends = written.iter().rev().zip(wanted.iter().rev());
        let shared = ends.take_while(same).count();
        if shared == 0 {
            continue;
        }
        if shared == written.len() && (shared == inside || shared == wanted.len()) {
            return Some(file);
        }
        // Of two reports that end alike, the one with the shorter module is the nearer.
        let rank = (shared, Reverse(written.len()));
        if go && shared >= inside.min(2) && best.is_none_or(|(most, _)| rank > most) {
            best = Some((rank, file));
        }
    }
    best.map(|(_, file)| file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LCOV: &str = "TN:\n\
        SF:/proj/src/lib.rs\n\
        FN:1,add\n\
        FNDA:3,add\n\
        FNF:1\n\
        FNH:1\n\
        DA:1,3\n\
        DA:2,3\n\
        DA:4,0\n\
        BRDA:2,0,0,1\n\
        LF:3\n\
        LH:2\n\
        end_of_record\n\
        TN:\n\
        SF:src\\main.rs\n\
        DA:1,1,Zm9vYmFy\n\
        DA:2,0\n\
        end_of_record\n\
        SF:/proj/src/lib.rs\n\
        DA:1,2\n\
        DA:9,1\n\
        end_of_record\n";

    const GO: &str = "mode: count\n\
        example.com/app/pkg/add.go:3.24,5.2 1 4\n\
        example.com/app/pkg/add.go:5.2,7.3 2 0\n\
        example.com/app/pkg/add.go:10.1,10.20 1 1\n\
        example.com/app/main.go:8.13,9.2 1 0\n";

    fn file(path: &str) -> FileCoverage {
        FileCoverage {
            path: path.to_owned(),
            lines: vec![(0, 1)],
        }
    }

    #[test]
    fn lines_that_ran_are_read_from_an_lcov_report() {
        let files = lcov(LCOV);
        assert_eq!(files.len(), 2);
        // The two records of the same file are one file, the hits of line 1 added up.
        assert_eq!(files[0].path, "/proj/src/lib.rs");
        assert_eq!(files[0].lines, [(0, 5), (1, 3), (3, 0), (8, 1)]);
        // A checksum after the hits is no trouble; the path stays as written.
        assert_eq!(files[1].path, "src\\main.rs");
        assert_eq!(files[1].lines, [(0, 1), (1, 0)]);
    }

    #[test]
    fn lines_of_an_lcov_report_that_cannot_be_read_are_skipped() {
        let report = "DA:1,1\n\
            SF:a.rs\n\
            DA:x,1\n\
            DA:2\n\
            DA:0,7\n\
            DA:3,-1\n\
            DA:4,1.5\n\
            DA:99999999999999999999999,1\n\
            DA:5,18446744073709551615\n\
            DA:5,9\n\
            what is this\n\
            DA: 6 , 2 \n\
            SF:\n\
            DA:7,1\n\
            end_of_record\n\
            DA:8,1\n\
            SF:b.rs\n\
            DA:1,1";
        let files = lcov(report);
        // Only the lines with a number from 1 and a whole count of hits are kept; hits
        // added past what a count can hold stay at the most it holds.
        assert_eq!(files[0].lines, [(4, u64::MAX), (5, 2)]);
        // A record the report never closes is read all the same.
        assert_eq!((files[1].path.as_str(), files.len()), ("b.rs", 2));
        assert_eq!(files[1].lines, [(0, 1)]);
    }

    #[test]
    fn the_share_of_lines_that_ran_is_a_percentage() {
        let files = lcov(LCOV);
        assert_eq!(files[0].percent(), 75.0);
        assert_eq!(files[1].percent(), 50.0);
        // Four lines ran out of the six known in both files.
        assert!((total_percent(&files) - 66.666_67).abs() < 0.001);
    }

    #[test]
    fn an_empty_report_has_no_file_and_no_percentage() {
        assert_eq!(lcov(""), []);
        assert_eq!(go_profile(""), []);
        assert_eq!(read(""), []);
        assert_eq!(total_percent(&[]), 0.0);
        // A file the report names without any line is known, at nothing.
        let named = lcov("SF:a.rs\nend_of_record\n");
        assert_eq!((named.len(), named[0].percent()), (1, 0.0));
    }

    #[test]
    fn every_line_of_a_go_block_takes_its_count() {
        let files = go_profile(GO);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "example.com/app/pkg/add.go");
        // Line 5 ends one block that ran and starts one that did not: it ran.
        assert_eq!(
            files[0].lines,
            [(2, 4), (3, 4), (4, 4), (5, 0), (6, 0), (9, 1)]
        );
        assert_eq!(files[1].lines, [(7, 0), (8, 0)]);
        // Four of the six lines of the first ran, four of the eight of both.
        assert!((files[0].percent() - 66.666_67).abs() < 0.001);
        assert_eq!((files[1].percent(), total_percent(&files)), (0.0, 50.0));
    }

    #[test]
    fn a_go_profile_is_read_in_its_three_modes() {
        for mode in ["set", "count", "atomic"] {
            let profile = format!("mode: {mode}\na.go:1.1,2.5 2 1\na.go:4.1,4.9 1 0\n");
            let files = go_profile(&profile);
            assert_eq!(files.len(), 1, "{mode}");
            assert_eq!(files[0].lines, [(0, 1), (1, 1), (3, 0)], "{mode}");
        }
    }

    #[test]
    fn lines_of_a_go_profile_that_cannot_be_read_are_skipped() {
        let profile = "mode: set\n\
            garbage\n\
            a.go:9.1,3.2 1 5\n\
            a.go:0.1,3.2 1 5\n\
            a.go:1.1,99999999999.2 1 5\n\
            a.go:1.1,2.2 1 many\n\
            a.go:1,2 1 1\n\
            :1.1,2.2 1 1\n\
            a.go:1.1,2.2 1\n\
            C:\\work\\my app\\b.go:2.1,2.9 1 3\n";
        let files = go_profile(profile);
        // A path may hold a drive and spaces; nothing else here is a block.
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "C:\\work\\my app\\b.go");
        assert_eq!(files[0].lines, [(1, 3)]);
    }

    #[test]
    fn the_report_of_a_file_is_found_however_its_path_is_written() {
        let root = Path::new("/proj");
        let wanted = Path::new("/proj/src/net/mod.rs");
        for written in [
            "/proj/src/net/mod.rs",
            "src/net/mod.rs",
            "./src/net/mod.rs",
            "src\\net\\mod.rs",
        ] {
            let all = [file("src/other.rs"), file(written)];
            let found = of_file(&all, wanted, root).map(|file| file.path.as_str());
            assert_eq!(found, Some(written));
            // The file may be given from the root too.
            let found = of_file(&all, Path::new("src/net/mod.rs"), root);
            assert_eq!(found.map(|file| file.path.as_str()), Some(written));
        }
        // On Windows: a drive, backslashes on either side, the `\\?\` some paths start with.
        let root = Path::new("C:\\proj");
        let wanted = Path::new("C:\\proj\\src\\main.rs");
        for written in [
            "C:\\proj\\src\\main.rs",
            "C:/proj/src/main.rs",
            "c:/proj/src/main.rs",
            "\\\\?\\C:\\proj\\src\\main.rs",
            "src/main.rs",
            "src\\main.rs",
        ] {
            let all = [file(written)];
            assert_eq!(of_file(&all, wanted, root), Some(&all[0]), "{written}");
        }
    }

    #[test]
    fn a_go_file_is_matched_by_the_longest_end_of_its_path() {
        let root = Path::new("/proj");
        let all = go_profile(
            "mode: set\n\
             example.com/app/cmd/tool/main.go:1.1,1.9 1 0\n\
             example.com/app/main.go:1.1,1.9 1 1\n\
             example.com/app/pkg/add.go:1.1,1.9 1 1\n",
        );
        let found =
            |path: &str| of_file(&all, Path::new(path), root).map(|file| file.path.as_str());
        // Both end in `main.go`: the one right under the module is the one at the root.
        assert_eq!(found("/proj/main.go"), Some("example.com/app/main.go"));
        assert_eq!(
            found("/proj/cmd/tool/main.go"),
            Some("example.com/app/cmd/tool/main.go")
        );
        assert_eq!(found("pkg/add.go"), Some("example.com/app/pkg/add.go"));
        // A module kept in a folder of the project: the folder and the name are enough.
        assert_eq!(
            found("/proj/backend/pkg/add.go"),
            Some("example.com/app/pkg/add.go")
        );
        assert_eq!(found("/proj/pkg/sub.go"), None);
        assert_eq!(found("/proj/other/add.go"), None);
    }

    #[test]
    fn a_file_the_report_does_not_know_has_no_report() {
        let root = Path::new("/proj");
        let all = [file("src/net/mod.rs"), file("/proj/src/lib.rs")];
        assert_eq!(of_file(&all, Path::new("/proj/src/main.rs"), root), None);
        // The same name in another folder is another file.
        assert_eq!(of_file(&all, Path::new("/proj/src/fs/mod.rs"), root), None);
        assert_eq!(of_file(&all, Path::new("/proj/lib.rs"), root), None);
        // Only Go names a file by the end of its path.
        let all = [file("crates/core/src/lib.rs"), file("/ci/proj/src/lib.rs")];
        let other = Path::new("/proj/crates/app/src/lib.rs");
        assert_eq!(of_file(&all, other, root), None);
        assert_eq!(of_file(&all, Path::new("/proj/src/lib.rs"), root), None);
        assert_eq!(of_file(&[], Path::new("/proj/src/lib.rs"), root), None);
    }

    #[test]
    fn the_format_of_a_report_is_told_from_its_text() {
        assert_eq!(format(LCOV), Some(Format::Lcov));
        assert_eq!(format(GO), Some(Format::GoProfile));
        assert_eq!(format("\n\n  mode: atomic\n"), Some(Format::GoProfile));
        assert_eq!(
            format("SF:a.rs\nDA:1,1\nend_of_record\n"),
            Some(Format::Lcov)
        );
        assert_eq!(format(""), None);
        assert_eq!(format("{ \"total\": 3 }"), None);
        // A report in either format is read without saying which.
        assert_eq!(read(LCOV), lcov(LCOV));
        assert_eq!(read(GO), go_profile(GO));
        assert_eq!(read("<coverage/>"), []);
    }
}
