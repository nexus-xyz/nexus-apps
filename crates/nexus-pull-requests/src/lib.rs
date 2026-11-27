//! Nexus for desktop's pull requests, with no UI toolkit: GitHub's pull requests and GitLab's merge requests read
//! through `gh` and `glab` ([`pull_request`]), the watch that tells the person and the agent what changed ([`watch`])
//! with the words a window shows for it ([`card`], [`badge`] for a sidebar row, [`chip`] for the top bar and
//! [`summary`] for the card beside a hovered row), a session's pull requests grouped into stacks ([`stack`]) and a
//! stack's rebase and merge ([`stack_actions`]), and the pull request page's reads and writes ([`pull_page`]). Nexus
//! GPUI uses it. The watch reaches the app through [`watch::Shell`],
//! and the sessions of the app's background service through [`over_service`]. A server session's pull request is read
//! by its repository's name ([`remote`]).

pub mod badge;
pub mod card;
pub mod chip;
pub mod over_service;
pub mod pull_page;
pub mod pull_request;
pub mod remote;
pub mod stack;
pub mod stack_actions;
pub mod summary;
pub mod watch;
