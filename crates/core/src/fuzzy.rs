//! Fuzzy matching of file paths, for "go to file".

const SEPARATORS: [char; 6] = ['/', '\\', '_', '-', '.', ' '];

/// Points for the letters of `query` found in order in `text`; `None` when some are missing.
fn subsequence(query: &str, text: &str) -> Option<i32> {
    let mut wanted = query.chars().map(|ch| ch.to_ascii_lowercase()).peekable();
    let mut points = 0;
    let mut previous: Option<char> = None;
    let mut previous_matched = false;
    for ch in text.chars() {
        let Some(next) = wanted.peek() else {
            break;
        };
        let matched = ch.to_ascii_lowercase() == *next;
        if matched {
            wanted.next();
            points += 1;
            if previous_matched {
                points += 8;
            }
            if previous.is_none_or(|before| SEPARATORS.contains(&before)) {
                points += 6;
            }
        }
        previous_matched = matched;
        previous = Some(ch);
    }
    wanted.peek().is_none().then_some(points)
}

/// How well `query` matches the path `candidate`: higher is better, `None` is no match.
/// Matches inside the file name beat matches spread over the folders.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    let name_start = candidate.rfind(['/', '\\']).map_or(0, |index| index + 1);
    let in_name = subsequence(query, &candidate[name_start..]).map(|points| points + 30);
    let anywhere = subsequence(query, candidate);
    let best = in_name.max(anywhere)?;
    Some(best - candidate.len() as i32 / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_must_appear_in_order() {
        assert!(score("mnrs", "src/main.rs").is_some());
        assert!(score("xyz", "src/main.rs").is_none());
        assert!(score("", "anything").is_some());
    }

    #[test]
    fn the_file_name_wins_over_folders() {
        let name = score("app", "crates/core/src/app.rs");
        let folder = score("app", "crates/app/src/lib.rs");
        assert!(name > folder);
    }

    #[test]
    fn consecutive_letters_win_over_scattered_ones() {
        assert!(score("edit", "src/editor.rs") > score("edit", "src/explorer_ditto.rs"));
    }
}
