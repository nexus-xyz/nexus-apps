//! One test binary for the crate, so it links once: a new test file needs its `mod` line below.

mod account;
mod agent_chat;
mod agent_protocol;
mod agent_share;
mod agents;
mod analytics;
mod artifacts;
mod billing;
mod budget;
mod budgets;
mod catalog;
mod connections;
mod dependencies;
mod design_systems;
mod device_login;
mod exports;
mod help;
mod jobs;
mod machines;
mod members;
mod no_account;
mod notifications;
mod open_build;
mod org_domains;
mod pages;
mod plan_tier;
#[cfg(feature = "plans")]
mod plans;
mod policy_controls;
mod provider_keys;
mod provider_names;
mod proxy_errors;
mod recipes;
mod review_status;
mod routines;
mod service_accounts;
#[cfg(unix)]
mod session_sync_e2e;
#[cfg(unix)]
mod sessions;
mod shared_memory;
mod skill_promotions;
mod skills;
mod slack;
mod tokens;
