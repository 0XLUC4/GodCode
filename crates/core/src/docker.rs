//! What Docker says of its containers and images, read from what its commands print.
//!
//! The app runs the commands; this reads their lines, one JSON object each, and writes
//! the command lines that act on a container. The services of a compose file are read too.

/// The command that lists the containers, stopped ones too, for [`containers`].
pub const LIST_CONTAINERS: &str = "docker ps -a --format \"{{json .}}\"";

/// The command that lists the images, for [`images`].
pub const LIST_IMAGES: &str = "docker images --format \"{{json .}}\"";

/// A container, as `docker ps` tells of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    /// As Docker words it: `Up 2 hours`, `Exited (0) 3 days ago`.
    pub status: String,
    /// As Docker words them: `0.0.0.0:5432->5432/tcp`.
    pub ports: String,
    pub running: bool,
}

/// An image, as `docker images` tells of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub id: String,
    /// `repository:tag`, or the repository alone when the image has no tag.
    pub name: String,
    /// As Docker words it: `432MB`.
    pub size: String,
    /// As Docker words it: `3 weeks ago`.
    pub created: String,
}

/// What can be done to a container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Remove,
    Logs,
    Shell,
}

/// A char Docker allows in an id or a name, and compose in the name of a service.
fn allowed(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || "_.-".contains(ch)
}

/// The four hex digits after `\u`.
fn hex(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    (0..4).try_fold(0, |code, _| Some(code * 16 + chars.next()?.to_digit(16)?))
}

/// Reads a JSON text between quotes, its escapes too, and goes past it.
fn text(rest: &mut &str) -> Option<String> {
    let mut chars = rest.trim_start().strip_prefix('"')?.chars();
    let mut text = String::new();
    loop {
        match chars.next()? {
            '"' => break,
            '\\' => text.push(match chars.next()? {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                'u' => {
                    let unit = hex(&mut chars)?;
                    // A char past U+FFFF is written as two halves, each its own `\u`.
                    let mut after = chars.clone();
                    let low = (after.next() == Some('\\') && after.next() == Some('u'))
                        .then(|| hex(&mut after))
                        .flatten();
                    let code = match low {
                        Some(low)
                            if (0xD800..0xDC00).contains(&unit)
                                && (0xDC00..0xE000).contains(&low) =>
                        {
                            chars = after;
                            0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
                        }
                        _ => unit,
                    };
                    char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER)
                }
                escaped @ ('"' | '\\' | '/') => escaped,
                _ => return None,
            }),
            ch => text.push(ch),
        }
    }
    *rest = chars.as_str();
    Some(text)
}

/// Goes past a value that is no text: a number, a word, an object or an array.
fn skip(rest: &mut &str) -> Option<()> {
    let start = rest.len();
    let mut depth = 0usize;
    loop {
        let mut chars = rest.chars();
        match chars.next()? {
            '"' => {
                text(rest)?;
                continue;
            }
            '{' | '[' => depth += 1,
            '}' | ']' | ',' if depth == 0 => return (rest.len() < start).then_some(()),
            '}' | ']' => depth -= 1,
            _ => {}
        }
        *rest = chars.as_str();
    }
}

/// The fields of a line that is one JSON object, each name with its value when that value
/// is a text. None when the line is no such object.
fn fields(line: &str) -> Option<Vec<(String, String)>> {
    let mut rest = line.trim().strip_prefix('{')?.trim_start();
    let mut fields = Vec::new();
    if let Some(after) = rest.strip_prefix('}') {
        return after.is_empty().then_some(fields);
    }
    loop {
        let name = text(&mut rest)?;
        rest = rest.trim_start().strip_prefix(':')?.trim_start();
        if rest.starts_with('"') {
            fields.push((name, text(&mut rest)?));
        } else {
            skip(&mut rest)?;
        }
        rest = rest.trim_start();
        match rest.strip_prefix(',') {
            Some(after) => rest = after,
            None => return rest.strip_prefix('}')?.is_empty().then_some(fields),
        }
    }
}

/// The value of a field, when the object has it.
fn field(fields: &[(String, String)], name: &str) -> Option<String> {
    let found = fields.iter().find(|(known, _)| known == name);
    found.map(|(_, value)| value.clone())
}

/// The containers [`LIST_CONTAINERS`] printed, in its order. A line that is no JSON object
/// with an `ID` is skipped: Docker may print a warning among them.
pub fn containers(printed: &str) -> Vec<Container> {
    let container = |line: &str| {
        let fields = fields(line)?;
        let status = field(&fields, "Status").unwrap_or_default();
        // An older Docker tells no state: its status then starts with `Up`.
        let running = match field(&fields, "State") {
            Some(state) => state == "running",
            None => status.starts_with("Up"),
        };
        Some(Container {
            id: field(&fields, "ID")?,
            name: field(&fields, "Names").unwrap_or_default(),
            image: field(&fields, "Image").unwrap_or_default(),
            status,
            ports: field(&fields, "Ports").unwrap_or_default(),
            running,
        })
    };
    printed.lines().filter_map(container).collect()
}

/// The images [`LIST_IMAGES`] printed, in its order. A line that is no JSON object with an
/// `ID` is skipped.
pub fn images(printed: &str) -> Vec<Image> {
    let image = |line: &str| {
        let fields = fields(line)?;
        let repository = field(&fields, "Repository").unwrap_or_default();
        let tag = field(&fields, "Tag").filter(|tag| !tag.is_empty() && tag != "<none>");
        Some(Image {
            id: field(&fields, "ID")?,
            name: match tag {
                Some(tag) => format!("{repository}:{tag}"),
                None => repository,
            },
            size: field(&fields, "Size").unwrap_or_default(),
            created: field(&fields, "CreatedSince").unwrap_or_default(),
        })
    };
    printed.lines().filter_map(image).collect()
}

/// The command line that does `action` to the container with this id or name.
///
/// The line goes to a shell: it is empty unless the id is made of letters, digits, `_`,
/// `.` and `-` only, and starts with a letter or a digit, so that it is no option either.
pub fn command(action: Action, id: &str) -> String {
    let safe = id.starts_with(|ch: char| ch.is_ascii_alphanumeric()) && id.chars().all(allowed);
    if !safe {
        return String::new();
    }
    match action {
        Action::Start => format!("docker start {id}"),
        Action::Stop => format!("docker stop {id}"),
        Action::Restart => format!("docker restart {id}"),
        Action::Remove => format!("docker rm {id}"),
        Action::Logs => format!("docker logs --tail 200 -f {id}"),
        Action::Shell => format!("docker exec -it {id} sh"),
    }
}

/// The key a line of YAML opens with, its quotes taken off: `web:`, `"web":`, `web: # note`.
/// None when the line opens with no key, or with one that could name no service.
fn key(line: &str) -> Option<&str> {
    let line = line.trim();
    let (key, after) = match line.chars().next()? {
        quote @ ('"' | '\'') => line[1..].split_once(quote)?,
        _ => line.split_at(line.find(':')?),
    };
    let after = after.trim_start().strip_prefix(':')?;
    let key = key.trim_end();
    let ends = after.is_empty() || after.starts_with(char::is_whitespace);
    (ends && !key.is_empty() && key.chars().all(allowed)).then_some(key)
}

/// The names of the services of a compose file, in the order written: the keys one level
/// under `services:`, itself written against the margin.
///
/// The file is read line by line, not as YAML: services written between braces
/// (`services: {web: ...}`), brought in by `<<:`, `include:` or `extends:`, or written in a
/// second document are not read. The level is the indentation of the first line under
/// `services:`, a tab counting as one space.
pub fn services(compose: &str) -> Vec<String> {
    let mut services = Vec::new();
    let mut inside = false;
    let mut level = None;
    for line in compose.trim_start_matches('\u{feff}').lines() {
        let written = line.trim_start();
        if written.is_empty() || written.starts_with('#') {
            continue;
        }
        let indent = line.len() - written.len();
        if indent == 0 {
            inside = key(line) == Some("services");
            level = None;
        } else if inside && indent == *level.get_or_insert(indent) {
            services.extend(key(line).map(str::to_owned));
        }
    }
    services
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = r#"{"Command":"\"docker-entrypoint.s…\"","CreatedAt":"2026-01-02 10:00:00 +0100 CET","ID":"3f2a9c1b7d5e","Image":"postgres:16","Labels":"","LocalVolumes":"1","Mounts":"pgdata","Names":"db","Networks":"bridge","Ports":"0.0.0.0:5432->5432/tcp","RunningFor":"2 hours ago","Size":"0B","State":"running","Status":"Up 2 hours"}"#;
    const WEB: &str = r#"{"Command":"\"/docker-entrypoint.…\"","CreatedAt":"2025-12-30 09:12:44 +0100 CET","ID":"9b1c0d2e3f4a","Image":"nginx:1.27","Labels":"com.docker.compose.project=shop,maintainer=NGINX Docker Maintainers \u003cdocker-maint@nginx.com\u003e","LocalVolumes":"0","Mounts":"","Names":"shop-web-1","Networks":"shop_default","Ports":"","RunningFor":"3 days ago","Size":"0B","State":"exited","Status":"Exited (0) 3 days ago"}"#;
    const POSTGRES: &str = r#"{"Containers":"N/A","CreatedAt":"2025-12-11 21:03:10 +0100 CET","CreatedSince":"3 weeks ago","Digest":"\u003cnone\u003e","ID":"a1b2c3d4e5f6","Repository":"postgres","SharedSize":"N/A","Size":"432MB","Tag":"16","UniqueSize":"N/A","VirtualSize":"431.9MB"}"#;
    const DANGLING: &str = r#"{"Containers":"N/A","CreatedAt":"2025-11-02 08:00:00 +0100 CET","CreatedSince":"2 months ago","Digest":"\u003cnone\u003e","ID":"0f9e8d7c6b5a","Repository":"\u003cnone\u003e","SharedSize":"N/A","Size":"1.2GB","Tag":"\u003cnone\u003e","UniqueSize":"N/A","VirtualSize":"1.2GB"}"#;

    fn only(line: &str) -> Container {
        let mut read = containers(line);
        assert_eq!(read.len(), 1, "{line}");
        read.remove(0)
    }

    #[test]
    fn the_containers_docker_lists_are_read() {
        let printed = format!("{DB}\n{WEB}\n");
        assert_eq!(
            containers(&printed),
            [
                Container {
                    id: "3f2a9c1b7d5e".to_owned(),
                    name: "db".to_owned(),
                    image: "postgres:16".to_owned(),
                    status: "Up 2 hours".to_owned(),
                    ports: "0.0.0.0:5432->5432/tcp".to_owned(),
                    running: true,
                },
                Container {
                    id: "9b1c0d2e3f4a".to_owned(),
                    name: "shop-web-1".to_owned(),
                    image: "nginx:1.27".to_owned(),
                    status: "Exited (0) 3 days ago".to_owned(),
                    ports: String::new(),
                    running: false,
                },
            ]
        );
        assert_eq!(containers(""), []);
    }

    #[test]
    fn a_container_runs_when_its_state_says_so_or_its_status_when_it_has_no_state() {
        assert!(only(r#"{"ID":"a","State":"running","Status":"Up 1 second"}"#).running);
        assert!(!only(r#"{"ID":"a","State":"exited","Status":"Exited (137) 1 hour ago"}"#).running);
        // The state wins over the status: a paused container is up, and does not run.
        assert!(!only(r#"{"ID":"a","State":"paused","Status":"Up 2 hours (Paused)"}"#).running);
        assert!(!only(r#"{"ID":"a","State":"","Status":"Up 2 hours"}"#).running);
        assert!(only(r#"{"ID":"a","Status":"Up 2 hours"}"#).running);
        assert!(!only(r#"{"ID":"a","Status":"Exited (0) 2 hours ago"}"#).running);
        assert!(!only(r#"{"ID":"a","Status":"Created"}"#).running);
        // A field that is missing is empty.
        let bare = only(r#"{"ID":"a"}"#);
        assert_eq!((bare.name.as_str(), bare.image.as_str()), ("", ""));
        assert_eq!((bare.status.as_str(), bare.ports.as_str()), ("", ""));
        assert!(!bare.running);
    }

    #[test]
    fn the_escapes_docker_writes_are_read() {
        // Docker writes `<`, `>` and `&` as `\u003c`, `\u003e` and `\u0026`.
        let line = r#"{"ID":"a","Names":"a\u0026b","Image":"\u003cnone\u003e","Status":"say \"hi\" \\ \/ \u00e9\ud83d\ude00\ud83d!","Ports":"a\tb\nc\r\b\f","State":"running"}"#;
        let read = only(line);
        assert_eq!(read.name, "a&b");
        assert_eq!(read.image, "<none>");
        assert_eq!(read.status, "say \"hi\" \\ / \u{e9}\u{1f600}\u{fffd}!");
        assert_eq!(read.ports, "a\tb\nc\r\u{8}\u{c}");
        // A name of field may be escaped too; chars past ASCII are read as written.
        let line = "{\"\\u0049D\":\"a\",\"Names\":\"caf\u{e9}\u{2026}\u{1f600}\"}";
        assert_eq!(only(line).name, "caf\u{e9}\u{2026}\u{1f600}");
    }

    #[test]
    fn a_line_that_is_no_container_is_skipped() {
        let printed = format!(
            "WARNING: Error loading config file: open config.json: permission denied\r\n\
             {DB}\r\n\r\n  \t\n[]\nnull\n\"ID\"\n{{}}\n{{\"Names\":\"no id\"}}\n{{\"ID\":7}}\n\
             {{\"ID\":\"a\"}} trailing\n{{\"ID\":\"a\",}}\n{{\"ID\":\"a\"\n{{\"ID\" \"a\"}}\n\
             {{\"ID\":\"a\" \"Names\":\"b\"}}\n{{ID:\"a\"}}\n{{\"ID\":'a'}}\n{{\"ID\":\"a\\x\"}}\n\
             {{\"ID\":\"\\u12\"}}\n{{\"ID\":\"a\",\"Names\":}}\n{{\"ID\":\"a\",\"Names\":[1,2}}\n\
             \t {{ \"ID\" : \"spaced\" , \"Names\" : \"s\" }} \n"
        );
        let ids: Vec<String> = containers(&printed)
            .into_iter()
            .map(|read| read.id)
            .collect();
        assert_eq!(ids, ["3f2a9c1b7d5e", "spaced"]);
        assert_eq!(images(&printed).len(), 2);
    }

    #[test]
    fn values_that_are_no_text_are_gone_past() {
        let line = r#"{"Size":12,"Labels":{"a":[1,{"b":"}]\""}],"c":null},"Mounts":["x","]",["y"]],"Up":true,"None":null,"Ratio":-1.5e+3,"Empty":[],"ID":"a","Names":"n"}"#;
        assert_eq!(only(line).name, "n");
        // A field read as a text elsewhere is empty when it is none here.
        let line = r#"{"ID":"a","Names":["n"],"Ports":80,"State":true,"Status":"Up"}"#;
        let read = only(line);
        assert_eq!((read.name.as_str(), read.ports.as_str()), ("", ""));
        assert!(read.running);
    }

    #[test]
    fn the_images_docker_lists_are_read() {
        let printed = format!("{POSTGRES}\n{DANGLING}\n");
        assert_eq!(
            images(&printed),
            [
                Image {
                    id: "a1b2c3d4e5f6".to_owned(),
                    name: "postgres".to_owned() + ":16",
                    size: "432MB".to_owned(),
                    created: "3 weeks ago".to_owned(),
                },
                Image {
                    id: "0f9e8d7c6b5a".to_owned(),
                    name: "<none>".to_owned(),
                    size: "1.2GB".to_owned(),
                    created: "2 months ago".to_owned(),
                },
            ]
        );
        let name = |line: &str| images(line).remove(0).name;
        assert_eq!(
            name(r#"{"ID":"a","Repository":"ghcr.io/me/app","Tag":"v1.2"}"#),
            "ghcr.io/me/app:v1.2"
        );
        assert_eq!(
            name(r#"{"ID":"a","Repository":"localhost:5000/app","Tag":"latest"}"#),
            "localhost:5000/app:latest"
        );
        assert_eq!(name(r#"{"ID":"a","Repository":"app"}"#), "app");
        assert_eq!(name(r#"{"ID":"a","Repository":"app","Tag":""}"#), "app");
        assert_eq!(name(r#"{"ID":"a","Tag":"v1"}"#), ":v1");
        assert_eq!(images(""), []);
        assert_eq!(
            images("REPOSITORY   TAG   IMAGE ID\n{\"Tag\":\"no id\"}"),
            []
        );
    }

    #[test]
    fn a_command_line_is_written_for_each_action() {
        assert_eq!(LIST_CONTAINERS, r#"docker ps -a --format "{{json .}}""#);
        assert_eq!(LIST_IMAGES, r#"docker images --format "{{json .}}""#);
        let id = "3f2a9c1b7d5e";
        assert_eq!(command(Action::Start, id), "docker start 3f2a9c1b7d5e");
        assert_eq!(command(Action::Stop, id), "docker stop 3f2a9c1b7d5e");
        assert_eq!(command(Action::Restart, id), "docker restart 3f2a9c1b7d5e");
        assert_eq!(command(Action::Remove, id), "docker rm 3f2a9c1b7d5e");
        assert_eq!(
            command(Action::Logs, id),
            "docker logs --tail 200 -f 3f2a9c1b7d5e"
        );
        assert_eq!(
            command(Action::Shell, id),
            "docker exec -it 3f2a9c1b7d5e sh"
        );
        assert_eq!(
            command(Action::Stop, "shop-web_1.old"),
            "docker stop shop-web_1.old"
        );
    }

    #[test]
    fn an_id_that_could_say_more_to_a_shell_gives_no_command() {
        let actions = [
            Action::Start,
            Action::Stop,
            Action::Restart,
            Action::Remove,
            Action::Logs,
            Action::Shell,
        ];
        let unsafe_ids = [
            "",
            " ",
            "a b",
            "a;rm -rf /",
            "a&&b",
            "a|b",
            "a&",
            "$(reboot)",
            "`reboot`",
            "$HOME",
            "a>b",
            "a<b",
            "a\nb",
            "a\r",
            "a\tb",
            "a\0b",
            "a'b",
            "a\"b",
            "a\\b",
            "a/b",
            "a:b",
            "a*",
            "a?",
            "~",
            "a%PATH%",
            "a^b",
            "a!b",
            "a#b",
            "a(b)",
            "a{b}",
            "a=b",
            "a,b",
            "a@b",
            " a",
            "a ",
            "caf\u{e9}",
            "\u{ff41}",
            "a\u{a0}b",
            "a\u{2028}b",
            // Allowed chars, but an option to docker, not a name.
            "-f",
            "--help",
            "--volumes",
            ".",
            "..",
            "_a",
        ];
        for id in unsafe_ids {
            for action in actions {
                assert_eq!(command(action, id), "", "{id:?}");
            }
        }
        // Whatever the id, a command holds nothing but the chars allowed.
        for id in ["db", "a.b-c_d", "A1", "0"] {
            for action in actions {
                let line = command(action, id);
                assert!(line.ends_with(id) || line.ends_with(&format!("{id} sh")));
                assert!(line.chars().all(|ch| allowed(ch) || ch == ' '), "{line}");
            }
        }
    }

    const COMPOSE: &str = "# the shop\nversion: \"3.9\"\n\nservices:\n  web:\n    image: nginx:1.27\n    ports:\n      - \"8080:80\"\n    environment:\n      services: nested\n    command: |\n      echo db:\n  # the database\n  db:\n    image: postgres:16\n\n  \"cache\":\n    image: redis\n  'worker-1': {image: busybox}\n  api.v2:   # note\n    build: .\n  job_1 :\n    image: busybox\n\nvolumes:\n  pgdata:\n";

    #[test]
    fn the_services_of_a_compose_file_are_read_in_order() {
        assert_eq!(
            services(COMPOSE),
            ["web", "db", "cache", "worker-1", "api.v2", "job_1"]
        );
        assert_eq!(services("services:\n  only:\n    image: x"), ["only"]);
        assert_eq!(services("services:\n  only:"), ["only"]);
    }

    #[test]
    fn line_ends_tabs_and_deeper_levels_are_tolerated() {
        let windows = format!("\u{feff}{}", COMPOSE.replace('\n', "\r\n"));
        assert_eq!(services(&windows), services(COMPOSE));
        let tabs = "services:\n\tweb:\n\t\timage: nginx\n\tdb:\n\t\timage: postgres\n";
        assert_eq!(services(tabs), ["web", "db"]);
        let four = "services: # all of them\n    web:\n        image: nginx\n    db:\n        image: postgres\n";
        assert_eq!(services(four), ["web", "db"]);
        let quoted = "\"services\" :\n  web: &web\n    image: nginx\n";
        assert_eq!(services(quoted), ["web"]);
        // Services after other keys, and before them.
        let last = "name: shop\nnetworks:\n  front:\nservices:\n  web:\n    image: nginx";
        assert_eq!(services(last), ["web"]);
    }

    #[test]
    fn what_is_no_service_is_not_read() {
        let none: [&str; 0] = [];
        assert_eq!(services(""), none);
        assert_eq!(services("volumes:\n  data:\n"), none);
        assert_eq!(services("services:\n"), none);
        assert_eq!(services("services:\nvolumes:\n  data:\n"), none);
        // Services under another key, or of another name.
        assert_eq!(services("x-shop:\n  services:\n    web:\n"), none);
        assert_eq!(services("  services:\n    web:\n"), none);
        assert_eq!(services("services-old:\n  web:\n"), none);
        assert_eq!(services("# services:\n  web:\n"), none);
        // Written between braces: not read.
        assert_eq!(services("services: {web: {image: nginx}}\n"), none);
        // Lines under `services:` that open with no name of service.
        let odd = "services:\n  <<: *shared\n  - item\n  ? complex\n  no colon\n  web:8080\n  a b: c\n  \"open: x\n  : x\n  real: {}\n";
        assert_eq!(services(odd), ["real"]);
        // A line less indented than the first is of no level, and is not read.
        let ragged = "services:\n    web:\n  half:\n    db:\n";
        assert_eq!(services(ragged), ["web", "db"]);
    }

    #[test]
    fn malformed_text_is_read_without_trouble() {
        for printed in [
            "{",
            "}",
            "{{",
            "{\"",
            "{\"\\",
            "{\"\\u",
            "{\"\\ud83d\\u",
            "{\"a\"",
            "{\"a\":",
            "{\"a\":\"",
            "{\"a\":[",
            "{\"a\":]",
            "{\"a\":}",
            "{\"a\":,",
            "{,}",
            "{:}",
            "\u{feff}{}",
            "{\"ID\":\"\\ud83d\\ud83d\"}",
            "{\"ID\":\"\\udc00\"}",
            "\u{e9}",
            "{\u{e9}}",
            "{\"\u{4e16}\":\u{754c}}",
        ] {
            containers(printed);
            images(printed);
            services(printed);
        }
        // What docker printed and a compose file, cut anywhere and read from anywhere.
        let odd =
            "{\"ID\":\"\u{e9}\\u00e9\\ud83d\\ude00\",\"L\":{\"a\":[1,\"\u{4e16}]\"]},\"N\":-1.5}";
        let quoted = "services:\n  \"caf\u{e9}\": x\n  '\u{4e16}':\n\t\u{e9}:\n  ok:\n";
        for text in [DB, WEB, POSTGRES, DANGLING, odd, COMPOSE, quoted] {
            for (at, _) in text.char_indices() {
                for part in [&text[..at], &text[at..]] {
                    containers(part);
                    images(part);
                    services(part);
                }
            }
        }
        assert_eq!(only(odd).id, "\u{e9}\u{e9}\u{1f600}");
        assert_eq!(services(quoted), ["ok"]);
        // Brackets nested past any depth are gone past without trouble.
        let deep = format!("{{\"ID\":\"a\",\"L\":{}", "[".repeat(200_000));
        assert_eq!(containers(&deep), []);
        let closed = format!(
            "{{\"L\":{}{},\"ID\":\"a\"}}",
            "[".repeat(200_000),
            "]".repeat(200_000)
        );
        assert_eq!(only(&closed).id, "a");
    }
}
