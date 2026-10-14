//! Local TCP ports that listen, and the programs that hold them: the end of
//! "port 3000 is already in use".

use std::collections::{BTreeMap, HashMap};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub port: u16,
    /// `127.0.0.1`, `0.0.0.0`, `[::]`…
    pub address: String,
    pub pid: u32,
    pub process: String,
}

impl Listener {
    /// Address to open in a browser.
    pub fn url(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// Reachable from other machines, not only from this one.
    pub fn public(&self) -> bool {
        !(self.address.starts_with("127.") || self.address == "[::1]" || self.address == "::1")
    }
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|err| format!("Cannot run {program}: {err}"))?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Every listening port, lowest first, one line per port and program.
pub fn listening() -> Result<Vec<Listener>, String> {
    if cfg!(windows) {
        let sockets = parse_netstat(&run("netstat", &["-ano", "-p", "TCP"])?)
            .into_iter()
            .chain(parse_netstat(&run("netstat", &["-ano", "-p", "TCPv6"])?));
        let names = parse_tasklist(&run("tasklist", &["/FO", "CSV", "/NH"])?);
        Ok(merge(sockets, &names))
    } else {
        let text = run("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN"])?;
        Ok(merge(parse_lsof(&text).into_iter(), &HashMap::new()))
    }
}

fn merge(
    sockets: impl Iterator<Item = (String, u16, u32, Option<String>)>,
    names: &HashMap<u32, String>,
) -> Vec<Listener> {
    // One port listens on IPv4 and IPv6 at once: show it once.
    let mut unique: BTreeMap<(u16, u32), Listener> = BTreeMap::new();
    for (address, port, pid, name) in sockets {
        let process = name
            .or_else(|| names.get(&pid).cloned())
            .unwrap_or_else(|| format!("PID {pid}"));
        let entry = unique.entry((port, pid)).or_insert_with(|| Listener {
            port,
            address: address.clone(),
            pid,
            process,
        });
        // Prefer the address that says it is open to the network.
        if !entry.public() && address != entry.address {
            entry.address = address;
        }
    }
    unique.into_values().collect()
}

/// `  TCP    0.0.0.0:135     0.0.0.0:0     LISTENING     1234`
fn parse_netstat(text: &str) -> Vec<(String, u16, u32, Option<String>)> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 5 || !fields[0].eq_ignore_ascii_case("TCP") {
                return None;
            }
            // The state is translated on some Windows versions; a listening socket has no peer.
            let peer_port = fields[2].rsplit(':').next()?;
            if peer_port != "0" && peer_port != "*" {
                return None;
            }
            let (address, port) = fields[1].rsplit_once(':')?;
            Some((
                address.to_owned(),
                port.parse().ok()?,
                fields[4].parse().ok()?,
                None,
            ))
        })
        .collect()
}

/// `"node.exe","4321","Console","1","52,180 K"`
fn parse_tasklist(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split("\",\"");
            let name = fields.next()?.trim_start_matches('"');
            let pid = fields.next()?.parse().ok()?;
            Some((pid, name.to_owned()))
        })
        .collect()
}

/// `node    4321 me   23u  IPv4 0x… 0t0  TCP *:3000 (LISTEN)`
fn parse_lsof(text: &str) -> Vec<(String, u16, u32, Option<String>)> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let name = fields.first()?;
            let pid = fields.get(1)?.parse().ok()?;
            let socket = fields.iter().rev().find(|field| field.contains(':'))?;
            let (address, port) = socket.rsplit_once(':')?;
            Some((
                address.to_owned(),
                port.parse().ok()?,
                pid,
                Some((*name).to_owned()),
            ))
        })
        .collect()
}

/// End the program that holds a port.
pub fn kill(pid: u32) -> Result<(), String> {
    if pid == std::process::id() {
        return Err("That is GodCode itself".into());
    }
    let pid_text = pid.to_string();
    let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
        ("taskkill", vec!["/PID", &pid_text, "/T", "/F"])
    } else {
        ("kill", vec!["-9", &pid_text])
    };
    let mut command = Command::new(program);
    command.args(&args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    let output = command
        .output()
        .map_err(|err| format!("Cannot run {program}: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if message.is_empty() {
            format!("Cannot end process {pid}")
        } else {
            message
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_netstat_and_tasklist() {
        let netstat = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1180
  TCP    127.0.0.1:3000         0.0.0.0:0              ABHÖREN         4321
  TCP    192.168.1.5:52000      140.82.112.4:443       ESTABLISHED     900
  TCP    [::]:3000              [::]:0                 LISTENING       4321
";
        let tasks = "\"svchost.exe\",\"1180\",\"Services\",\"0\",\"9,000 K\"\n\"node.exe\",\"4321\",\"Console\",\"1\",\"52,180 K\"\n";
        let listeners = merge(parse_netstat(netstat).into_iter(), &parse_tasklist(tasks));
        assert_eq!(listeners.len(), 2);
        assert_eq!(listeners[0].port, 135);
        assert_eq!(listeners[0].process, "svchost.exe");
        assert_eq!(listeners[1].port, 3000);
        assert_eq!(listeners[1].process, "node.exe");
        assert_eq!(listeners[1].address, "[::]", "the open address wins");
        assert!(listeners[1].public());
        assert_eq!(listeners[1].url(), "http://localhost:3000");
    }

    #[test]
    fn reads_lsof() {
        let text = "COMMAND  PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\nnode    4321 me   23u IPv4 0xabc 0t0 TCP 127.0.0.1:5173 (LISTEN)\n";
        let listeners = merge(parse_lsof(text).into_iter(), &HashMap::new());
        assert_eq!(listeners[0].port, 5173);
        assert_eq!(listeners[0].process, "node");
        assert!(!listeners[0].public());
    }

    #[test]
    fn will_not_kill_itself() {
        assert!(kill(std::process::id()).is_err());
    }
}
