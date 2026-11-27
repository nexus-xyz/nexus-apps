//! Import logins from a password manager's CSV export: Chrome, 1Password and Bitwarden.
//!
//! The file is read on this device and parsed here. Its rows become [`NewLogin`]s for the Nexus vault in the keychain.
//! An error names a row's number and never its content: a row holds a password.

use crate::vault_local::{NewLogin, Secret, site_of};

/// The largest export read, in bytes.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Where an export came from, by its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Chrome,
    OnePassword,
    Bitwarden,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Chrome => "Chrome",
            Format::OnePassword => "1Password",
            Format::Bitwarden => "Bitwarden",
        }
    }
}

/// What a file came to.
pub struct Parsed {
    pub format: Format,
    pub logins: Vec<NewLogin>,
    /// Rows that were not logins, or lacked a site or a password.
    pub skipped: usize,
}

impl std::fmt::Debug for Parsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parsed")
            .field("format", &self.format)
            .field("logins", &self.logins.len())
            .field("skipped", &self.skipped)
            .finish()
    }
}

/// Why a file was not read. No variant quotes the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    TooLarge,
    NotText,
    /// No header that names a site, a username and a password.
    NotAnExport,
    /// A quote that never closes.
    Broken,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ImportError::TooLarge => "That file is larger than 8 MB, so it is not a password export.",
            ImportError::NotText => "That file is not text. Choose the CSV your password manager exported.",
            ImportError::NotAnExport => {
                "That CSV has no password columns. Export from Chrome, 1Password or Bitwarden and choose that file."
            }
            ImportError::Broken => "That CSV ends inside a quote. Export it again.",
        })
    }
}

/// RFC 4180 rows: quotes, doubled quotes, commas and line breaks inside quotes, CRLF, and a leading byte order mark.
fn rows(text: &str) -> Result<Vec<Vec<String>>, ImportError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    while let Some(c) = chars.next() {
        any = true;
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cell.push(c);
            }
            continue;
        }
        match c {
            '"' if cell.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                out.push(std::mem::take(&mut row));
                any = false;
            }
            other => cell.push(other),
        }
    }
    if quoted {
        return Err(ImportError::Broken);
    }
    if any || !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        out.push(row);
    }
    out.retain(|row| !(row.len() == 1 && row[0].is_empty()));
    Ok(out)
}

fn column(header: &[String], names: &[&str]) -> Option<usize> {
    header
        .iter()
        .position(|cell| names.contains(&cell.trim().to_ascii_lowercase().as_str()))
}

/// Parse an export's text.
pub fn parse(text: &str) -> Result<Parsed, ImportError> {
    if text.len() > MAX_BYTES {
        return Err(ImportError::TooLarge);
    }
    let mut table = rows(text)?.into_iter();
    let header = table.next().ok_or(ImportError::NotAnExport)?;
    let name = column(&header, &["name", "title"]);
    let url = column(&header, &["url", "login_uri", "website", "web site"]);
    let user = column(&header, &["username", "login_username", "user name"]);
    let pass = column(&header, &["password", "login_password"]);
    let kind = column(&header, &["type"]);
    let (Some(pass), Some(_)) = (pass, user.or(url)) else {
        return Err(ImportError::NotAnExport);
    };
    let format = if column(&header, &["login_uri"]).is_some() {
        Format::Bitwarden
    } else if column(&header, &["title"]).is_some() {
        Format::OnePassword
    } else {
        Format::Chrome
    };
    let cell = |row: &[String], at: Option<usize>| {
        at.and_then(|at| row.get(at))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let mut logins = Vec::new();
    let mut skipped = 0;
    for row in table {
        // Bitwarden and 1Password mix cards, notes and identities into the file: only logins are kept.
        let not_login = kind.is_some() && {
            let kind = cell(&row, kind).to_ascii_lowercase();
            !kind.is_empty() && kind != "login"
        };
        let password = row.get(pass).cloned().unwrap_or_default();
        let site = site_of(&cell(&row, url)).or_else(|| site_of(&cell(&row, name)).filter(|site| site.contains('.')));
        match (not_login, site, password.is_empty()) {
            (false, Some(site), false) => {
                let title = cell(&row, name);
                logins.push(NewLogin {
                    name: if title.is_empty() { site.clone() } else { title },
                    site,
                    username: cell(&row, user),
                    password: Secret::new(password),
                });
            }
            _ => skipped += 1,
        }
    }
    Ok(Parsed {
        format,
        logins,
        skipped,
    })
}

/// Parse an export's bytes.
pub fn parse_bytes(bytes: &[u8]) -> Result<Parsed, ImportError> {
    if bytes.len() > MAX_BYTES {
        return Err(ImportError::TooLarge);
    }
    parse(std::str::from_utf8(bytes).map_err(|_| ImportError::NotText)?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn chrome_export() {
        let csv = "name,url,username,password,note\nDelta,https://www.delta.com/login,maya,\"p,w\"\"1\",\n,,,,\nNoPass,https://x.com,u,,\n";
        let parsed = parse(csv).unwrap();
        assert_eq!(parsed.format, Format::Chrome);
        assert_eq!(parsed.logins.len(), 1);
        assert_eq!(
            (parsed.logins[0].site.as_str(), parsed.logins[0].password.expose()),
            ("delta.com", "p,w\"1")
        );
        assert_eq!(parsed.skipped, 2);
    }

    #[test]
    fn onepassword_export_with_crlf_and_bom() {
        let csv = "\u{feff}Title,Url,Username,Password,OTPAuth,Favorite,Archived,Tags,Notes\r\nGitHub,https://github.com,maya,pw-1,,0,0,,\"line\r\ntwo\"\r\n";
        let parsed = parse(csv).unwrap();
        assert_eq!(parsed.format, Format::OnePassword);
        assert_eq!(parsed.logins[0].name, "GitHub");
    }

    #[test]
    fn bitwarden_export_keeps_logins_only() {
        let csv = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n,,login,Bank,,,0,https://bank.example,maya,pw-b,\n,,note,A note,secret text,,0,,,,\n";
        let parsed = parse(csv).unwrap();
        assert_eq!(parsed.format, Format::Bitwarden);
        assert_eq!((parsed.logins.len(), parsed.skipped), (1, 1));
        assert_eq!(parsed.logins[0].username, "maya");
    }

    #[test]
    fn errors_never_quote_the_file() {
        for (csv, error) in [
            ("a,b\n1,2\n", ImportError::NotAnExport),
            ("name,url,username,password\n\"x,y,Zecret-pw\n", ImportError::Broken),
        ] {
            let found = parse(csv).unwrap_err();
            assert_eq!(found, error);
            assert!(!format!("{found} {found:?}").contains("Zecret-pw"));
        }
        assert_eq!(parse_bytes(&[0xff, 0xfe, 0x00]).unwrap_err(), ImportError::NotText);
        let parsed = parse("name,url,username,password\nA,a.com,u,Zecret-pw-9\n").unwrap();
        assert!(!format!("{parsed:?}").contains("Zecret-pw-9"));
    }
}
