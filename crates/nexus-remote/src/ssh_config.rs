//! The hosts a person's `~/.ssh/config` names (and the files it includes), for the "Add a server" list. Patterns and
//! negations are left out. Nexus GPUI's Servers page reads them here.

use std::path::{Path, PathBuf};

use crate::ssh::valid_destination;

/// How many `Include` levels are followed.
const MAX_DEPTH: u8 = 4;

/// The hosts `~/.ssh/config` names under `home`, in order, each once.
pub fn hosts(home: &Path) -> Vec<String> {
    let dir = home.join(".ssh");
    let mut hosts = vec![];
    read(&dir.join("config"), &dir, home, &mut hosts, 0);
    hosts
}

fn read(path: &Path, dir: &Path, home: &Path, hosts: &mut Vec<String>, depth: u8) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let config = entries(&text);
    for host in config.hosts {
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    if depth >= MAX_DEPTH {
        return;
    }
    for include in config.includes {
        for file in include_files(&include, dir, home) {
            read(&file, dir, home, hosts, depth + 1);
        }
    }
}

/// An ssh config's `Host` names (no patterns, no negations) and `Include` paths, in order.
#[derive(Debug, Default, PartialEq)]
pub struct SshConfigEntries {
    pub hosts: Vec<String>,
    pub includes: Vec<String>,
}

pub fn entries(text: &str) -> SshConfigEntries {
    let mut entries = SshConfigEntries::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let (key, rest) = line
            .split_once(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or((line, ""));
        let values = rest
            .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
            .split_whitespace();
        if key.eq_ignore_ascii_case("host") {
            for host in values {
                let host = host.trim_matches('"');
                if !host.contains(['*', '?', '!'])
                    && valid_destination(host)
                    && !entries.hosts.iter().any(|h| h == host)
                {
                    entries.hosts.push(host.to_string());
                }
            }
        } else if key.eq_ignore_ascii_case("include") {
            entries.includes.extend(values.map(|v| v.trim_matches('"').to_string()));
        }
    }
    entries
}

/// The files an `Include` names: `~` is home, a relative path is under `~/.ssh`, and `*` matches within the last part.
fn include_files(pattern: &str, dir: &Path, home: &Path) -> Vec<PathBuf> {
    let path = if let Some(rest) = pattern.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        dir.join(pattern)
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let mut files: Vec<PathBuf> = path
        .parent()
        .and_then(|parent| std::fs::read_dir(parent).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|file| {
            let n = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            wildcard_match(&name, &n) && file.is_file()
        })
        .collect();
    files.sort();
    files
}

/// `ssh_config` wildcard match: `*` is any run of characters, `?` is exactly one.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_match_many_stars_and_question_marks() {
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("a*b*c", "aXXbYYc"));
        assert!(!wildcard_match("a*b*c", "aXXbYY"));
        assert!(wildcard_match("conf?.d", "conf1.d"));
        assert!(!wildcard_match("conf?.d", "conf.d"));
        assert!(wildcard_match("*.conf", "x.conf"));
        assert!(!wildcard_match("*.conf", "x.conf.bak"));
        assert!(wildcard_match("", ""));
    }

    #[test]
    fn reads_the_hosts_an_ssh_config_names() {
        let entries = entries(
            "# mine\nInclude ~/.orbstack/ssh/config\nHost vps vps-old\n  HostName 1.2.3.4\nHost *.internal !bad\nhost=box\nHost \"quoted\"\nMatch all\n",
        );
        assert_eq!(entries.hosts, vec!["vps", "vps-old", "box", "quoted"]);
        assert_eq!(entries.includes, vec!["~/.orbstack/ssh/config"]);
    }

    #[test]
    fn follows_includes_under_a_home() {
        let home = std::env::temp_dir().join(format!("nexus-remote-ssh-config-{}", std::process::id()));
        let ssh = home.join(".ssh");
        let _ = std::fs::create_dir_all(ssh.join("conf.d"));
        let _ = std::fs::write(ssh.join("config"), "Include conf.d/*.conf\nHost vps\n");
        let _ = std::fs::write(ssh.join("conf.d/a.conf"), "Host box vps\n");
        let _ = std::fs::write(ssh.join("conf.d/b.txt"), "Host skipped\n");
        assert_eq!(hosts(&home), vec!["vps", "box"]);
        let _ = std::fs::remove_dir_all(&home);
    }
}
