use crate::Scope;
use std::path::{Path, PathBuf};
use std::process::Command;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository(String);

impl Repository {
    pub fn discover(cwd: &Path) -> Option<Self> {
        let common = git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        // A relative origin is shared in the common Git config. Resolve it from the main checkout.
        let git_dir = Path::new(&common);
        let primary = git_dir.parent().filter(|_| git_dir.ends_with(".git")).unwrap_or(cwd);
        let key = git(cwd, &["remote", "get-url", "origin"])
            .and_then(|remote| origin(primary, &remote))
            .unwrap_or_else(|| format!("git:{}", common));
        Some(Self(key))
    }

    pub fn worktrees(cwd: &Path) -> Vec<PathBuf> {
        git(cwd, &["worktree", "list", "--porcelain", "-z"])
            .into_iter()
            .flat_map(|output| {
                output
                    .split('\0')
                    .filter_map(|part| part.strip_prefix("worktree ").map(PathBuf::from))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn checkout(cwd: &Path) -> Option<PathBuf> {
        git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
    }

    pub fn fingerprint(&self) -> String {
        crate::hash(&self.0)
    }

    pub fn scope(&self) -> Scope {
        Scope::Repository(self.0.clone())
    }
}

pub(crate) fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // Windows CREATE_NO_WINDOW.
    }
    let output = command.current_dir(cwd).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout)
        .ok()?
        .trim_end_matches(['\n', '\r'])
        .to_owned();
    (!text.is_empty()).then_some(text)
}

pub(crate) fn origin(cwd: &Path, remote: &str) -> Option<String> {
    let normalize = |host: &str, port: Option<u16>, path: &str| {
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let host = host.to_ascii_lowercase();
        let path = if matches!(host.as_str(), "github.com" | "gitlab.com") {
            path.to_ascii_lowercase()
        } else {
            path.to_owned()
        };
        let host = match port {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        format!("remote:{host}/{path}")
    };
    if let Ok(url) = Url::parse(remote) {
        if let Some(host) = url.host_str().filter(|_| url.scheme() != "file") {
            let port = url
                .port()
                .filter(|port| !matches!((url.scheme(), port), ("ssh", 22) | ("git", 9418)));
            return Some(normalize(host, port, url.path()));
        }
        if let Ok(path) = url.to_file_path() {
            return Some(local_origin(path));
        }
    }
    let path = Path::new(remote);
    if !path.is_absolute()
        && let Some((address, path)) = remote.split_once(':')
        && !address.contains(['/', '\\'])
        && !path.is_empty()
    {
        let host = address.rsplit('@').next()?;
        return Some(normalize(host, None, path));
    }
    Some(local_origin(cwd.join(path)))
}

fn local_origin(path: PathBuf) -> String {
    let path = path.canonicalize().unwrap_or(path);
    format!("file:{}", path.display())
}
