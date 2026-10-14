//! Secrets left in code: API keys, tokens, private keys, passwords written in clear.
//! Checked before a commit and on demand over the whole project.

use std::sync::OnceLock;

use regex::Regex;

/// A line that holds this marker is never reported (a test key, a documented example).
pub const ALLOW_MARKER: &str = "godcode:allow-secret";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// 1-based.
    pub line: usize,
    /// 1-based, in characters.
    pub column: usize,
    pub kind: &'static str,
    /// The secret with its middle hidden, safe to show on screen.
    pub masked: String,
}

struct Rule {
    kind: &'static str,
    pattern: Regex,
    /// Index of the group that holds the secret itself (0: the whole match).
    group: usize,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |kind, pattern: &str, group| Rule {
            kind,
            pattern: Regex::new(pattern).expect("valid secret pattern"),
            group,
        };
        vec![
            rule("Private key", r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----", 0),
            rule("AWS access key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", 0),
            rule("GitHub token", r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b", 0),
            rule("GitHub token", r"\bgithub_pat_[A-Za-z0-9_]{60,}\b", 0),
            rule("GitLab token", r"\bglpat-[A-Za-z0-9_\-]{20}\b", 0),
            rule("Anthropic API key", r"\bsk-ant-[A-Za-z0-9_\-]{30,}", 0),
            rule("OpenAI API key", r"\bsk-(?:proj-|svcacct-)?[A-Za-z0-9_\-]{32,}", 0),
            rule("Google API key", r"\bAIza[0-9A-Za-z_\-]{35}\b", 0),
            rule("Stripe secret key", r"\b(?:sk|rk)_live_[0-9A-Za-z]{20,}\b", 0),
            rule("Slack token", r"\bxox[abprs]-[0-9A-Za-z\-]{10,}", 0),
            rule("Slack webhook", r"https://hooks\.slack\.com/services/[A-Za-z0-9/]{30,}", 0),
            rule("Discord webhook", r"https://(?:ptb\.|canary\.)?discord(?:app)?\.com/api/webhooks/\d+/[A-Za-z0-9_\-]{30,}", 0),
            rule("npm token", r"\bnpm_[A-Za-z0-9]{36}\b", 0),
            rule("Hugging Face token", r"\bhf_[A-Za-z0-9]{34,}\b", 0),
            rule("SendGrid key", r"\bSG\.[A-Za-z0-9_\-]{22}\.[A-Za-z0-9_\-]{43}\b", 0),
            rule("Password in a URL", r"\b[a-z][a-z0-9+.\-]*://[^\s:/@]+:([^\s:/@]{6,})@", 1),
            rule(
                "Secret in clear",
                r#"(?i)\b[a-z0-9_.\-]*(?:password|passwd|secret|api_?key|access_?token|auth_?token|private_?key|client_?secret)[a-z0-9_.\-]*["']?\s*[:=]\s*["']([^"'\s]{8,})["']"#,
                1,
            ),
        ]
    })
}

/// Values that only stand for a secret: `"changeme"`, `"${API_KEY}"`, `"<your key>"`.
fn placeholder(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let words = [
        "example",
        "changeme",
        "change_me",
        "your",
        "xxxx",
        "****",
        "placeholder",
        "dummy",
        "sample",
        "redacted",
        "todo",
        "secret",
        "password",
        "test",
        "fake",
    ];
    value.starts_with(['$', '<', '{', '%'])
        || value.contains("{{")
        || words.iter().any(|word| lower.contains(word))
        || value
            .chars()
            .all(|ch| ch == value.chars().next().unwrap_or(' '))
}

/// `sk-ant-abcdef…uvwx`: enough to recognise it, not enough to use it.
fn mask(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 12 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..6].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

pub fn scan(text: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.contains(ALLOW_MARKER) || line.len() > 4000 {
            continue;
        }
        for rule in rules() {
            for captures in rule.pattern.captures_iter(line) {
                let Some(found) = captures.get(rule.group) else {
                    continue;
                };
                if rule.group > 0 && placeholder(found.as_str()) {
                    continue;
                }
                let column = line[..found.start()].chars().count() + 1;
                let duplicate = findings
                    .iter()
                    .any(|other: &Finding| other.line == index + 1 && other.column == column);
                if !duplicate {
                    findings.push(Finding {
                        line: index + 1,
                        column,
                        kind: rule.kind,
                        masked: mask(found.as_str()),
                    });
                }
            }
        }
    }
    findings
}

/// Files that hold secrets by design and are meant to stay out of Git.
pub fn is_secret_file(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    let is_env = name == ".env"
        || (name.starts_with(".env.")
            && !name.ends_with(".example")
            && !name.ends_with(".sample")
            && !name.ends_with(".template"));
    is_env
        || ["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"].contains(&name.as_str())
        || [".pem", ".key", ".p12", ".pfx", ".keystore", ".jks"]
            .iter()
            .any(|ext| name.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tokens_and_keys() {
        let text = "\
let key = \"AKIAIOSFODNN7EXAMPLQ\";
token: ghp_abcdefghijklmnopqrstuvwxyz0123456789
const client = new Anthropic({ apiKey: \"sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789\" });
-----BEGIN OPENSSH PRIVATE KEY-----
DATABASE_URL=postgres://admin:hunter22x@db.example.com/app
";
        let found = scan(text);
        let kinds: Vec<&str> = found.iter().map(|finding| finding.kind).collect();
        assert_eq!(
            kinds,
            [
                "AWS access key",
                "GitHub token",
                "Anthropic API key",
                "Private key",
                "Password in a URL"
            ]
        );
        assert_eq!(found[1].line, 2);
        assert_eq!(found[1].column, 8);
        assert_eq!(found[1].masked, "ghp_ab…6789");
    }

    #[test]
    fn finds_passwords_written_in_clear() {
        let found = scan(r#"db_password = "Tr0ub4dor&3x""#);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "Secret in clear");
        assert_eq!(found[0].masked, "••••••••••••");
    }

    #[test]
    fn leaves_placeholders_and_allowed_lines() {
        let text = r#"
password = "changeme123"
api_key = "${API_KEY}"
secret: "<your secret here>"
let fake = "ghp_abcdefghijklmnopqrstuvwxyz0123456789"; // godcode:allow-secret
let label = "Password";
"#;
        assert_eq!(scan(text), Vec::new());
    }

    #[test]
    fn secret_files() {
        assert!(is_secret_file(".env"));
        assert!(is_secret_file("config/.env.production"));
        assert!(!is_secret_file(".env.example"));
        assert!(is_secret_file("certs/server.key"));
        assert!(!is_secret_file("src/keyboard.rs"));
    }
}
