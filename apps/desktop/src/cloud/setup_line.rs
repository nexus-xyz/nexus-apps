//! Why cloud machines can't run for the person now, as a setup line (`crate::setup`): the card in the composer's place
//! on the Code screen and the top of Settings › Cloud. Each state carries its fix: sign in, ask the admins who can
//! change a role, or try the read again. Nobody in the organization can turn cloud machines on: Nexus does, for each
//! organization (`NEXUS_CLOUD_TENANTS`), so that state is quiet text.

use gpui_kit::SharedString;
use gpui_kit::assets::IconName as Lucide;

use super::CloudAccount;
use crate::pages::PageId;
use crate::setup::{Fix, SetupLine, SignIn, Tone};

/// The blocked action: a cloud start. [`crate::setup::done`] on it once cloud machines are available again.
pub const KEY: &str = "cloud:unavailable";

/// What keeps cloud machines from the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// This Nexus offers them to nobody, or not to this organization: only Nexus can change that.
    Off,
    /// The person's role in the organization may not use them.
    Role,
    /// No Nexus sign-in for the cloud machines on this computer.
    SignedOut,
    /// Gateway or the provider failed to answer: why, in plain words.
    Failed(SharedString),
}

impl Blocker {
    /// The blocker of an account read: `None` when cloud machines are available.
    pub fn of(account: &Result<CloudAccount, SharedString>) -> Option<Blocker> {
        match account {
            Err(error) => Some(Blocker::Failed(error.clone())),
            Ok(account) if account.availability.available => None,
            Ok(account) => Some(Blocker::from_reason(account.availability.reason.as_deref())),
        }
    }

    /// `CloudAvailability::reason`, as a blocker.
    pub fn from_reason(reason: Option<&str>) -> Blocker {
        match reason {
            Some("not_allowed") => Blocker::Role,
            Some("signed_out") => Blocker::SignedOut,
            Some("degraded") => Blocker::Failed(TROUBLE.into()),
            _ => Blocker::Off,
        }
    }
}

/// Why a read failed when Gateway or the provider struggles (503).
const TROUBLE: &str = "Cloud machines are having trouble right now. Nexus keeps trying.";

/// The line for `blocker` in organization `org` (its display name). `retry` reads the account again.
pub fn line(blocker: &Blocker, org: &str, retry: Fix) -> SetupLine {
    let what = what(blocker, org);
    match blocker {
        Blocker::Off => SetupLine::new(KEY, Lucide::Cloud, what)
            .why("Nexus turns them on for each organization.")
            .tone(Tone::Info),
        Blocker::Role => SetupLine::new(KEY, Lucide::Cloud, what)
            .fix("Change role", Fix::open(PageId::Members).needs("members.manage"))
            .ask(
                format!("Could you let me use cloud machines in {org}? My role can't use them now."),
                "change your role",
            )
            .main(),
        Blocker::SignedOut => SetupLine::new(KEY, Lucide::Cloud, what)
            .why("Sign in once on this computer.")
            .fix("Sign in", Fix::SignIn(SignIn::Nexus))
            .main(),
        Blocker::Failed(why) => SetupLine::new(KEY, Lucide::Cloud, what)
            .why(why.clone())
            .tone(Tone::Failed)
            .fix("Try again", retry)
            .main(),
    }
}

/// The line's first sentence: what is missing, for whom.
fn what(blocker: &Blocker, org: &str) -> String {
    match blocker {
        Blocker::Off => format!("Cloud machines aren't on for {org}."),
        Blocker::Role => format!("Your role in {org} can't use cloud machines."),
        Blocker::SignedOut => "Cloud machines need your Nexus sign-in.".into(),
        Blocker::Failed(_) => format!("Couldn't read {org}'s cloud account."),
    }
}

/// A forced read: the list asked again (`synced`), then the account. A failed list shows as a failed read only while
/// the account still says available; a refusal it understood is already the account's availability.
pub fn read_again(synced: Result<(), String>, account: CloudAccount) -> Result<CloudAccount, SharedString> {
    match synced {
        Err(error) if account.availability.available => Err(plain(&error).into()),
        _ => Ok(account),
    }
}

/// A failed read's reason in plain words: the first sentence, with a full stop.
fn plain(error: &str) -> String {
    let text = error.trim();
    if text.is_empty() {
        return "Nexus didn't answer.".into();
    }
    if text.ends_with(['.', '!', '?']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::CloudAccount;
    use core::prelude::v1::test;
    use nexus_cloud::desktop::{ClaudeAccount, CloudAvailability, GithubAccount};

    fn account(reason: Option<&str>) -> CloudAccount {
        CloudAccount {
            availability: CloudAvailability {
                available: reason.is_none(),
                reason: reason.map(str::to_string),
            },
            claude: ClaudeAccount {
                connected: false,
                hint: None,
                saved_at: None,
            },
            github: GithubAccount {
                connected: false,
                user: None,
                saved_at: None,
            },
        }
    }

    #[test]
    fn each_reason_has_its_blocker() {
        assert_eq!(Blocker::of(&Ok(account(None))), None);
        assert_eq!(Blocker::of(&Ok(account(Some("unconfigured")))), Some(Blocker::Off));
        assert_eq!(Blocker::of(&Ok(account(Some("not_allowed")))), Some(Blocker::Role));
        assert_eq!(Blocker::of(&Ok(account(Some("signed_out")))), Some(Blocker::SignedOut));
        assert_eq!(
            Blocker::of(&Ok(account(Some("degraded")))),
            Some(Blocker::Failed(TROUBLE.into()))
        );
        assert_eq!(
            Blocker::of(&Err("Nexus didn't answer.".into())),
            Some(Blocker::Failed("Nexus didn't answer.".into()))
        );
    }

    #[test]
    fn a_failed_list_is_a_failed_read_only_while_the_account_says_available() {
        assert_eq!(
            read_again(Err("Nexus did not answer in time".into()), account(None)),
            Err("Nexus did not answer in time.".into())
        );
        // A refusal the cloud machines understood is the account's availability already.
        assert_eq!(
            read_again(Err("refused".into()), account(Some("not_allowed"))),
            Ok(account(Some("not_allowed")))
        );
        assert_eq!(read_again(Ok(()), account(None)), Ok(account(None)));
    }

    #[test]
    fn the_first_words_say_what_is_missing_and_name_the_organization() {
        assert_eq!(what(&Blocker::Off, "Acme"), "Cloud machines aren't on for Acme.");
        assert_eq!(
            what(&Blocker::Role, "Acme"),
            "Your role in Acme can't use cloud machines."
        );
        assert_eq!(
            what(&Blocker::Failed("x.".into()), "Acme"),
            "Couldn't read Acme's cloud account."
        );
        let line = line(&Blocker::SignedOut, "Acme", Fix::act(|_, _| {}));
        assert_eq!(line.key().as_ref(), KEY, "every state keys the blocked cloud start");
    }
}
