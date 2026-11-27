//! Whether the person signed in is one of Nexus's own people. The CLIs keep `--env dev` for them, and
//! CLI launchers add the commit trailer hook only for them. Desktop includes every Nexus organization
//! member through [`crate::commit_trailer::desktop_eligible`].

use serde_json::Value;

/// Whether `GET /v1/tenants/me` names one of Nexus's own people: marked `staff` on Gateway, or signed in
/// with a Nexus Google Workspace address, which covers people not marked yet.
pub fn is_employee(me: &Value) -> bool {
    let user = |field: &str| me.pointer(&format!("/user/{field}")).and_then(Value::as_str);
    user("mark") == Some("staff")
        || user("email").is_some_and(|email| {
            let email = email.to_ascii_lowercase();
            ["@nexus.xyz", "@nexus.inc"]
                .iter()
                .any(|domain| email.ends_with(domain))
        })
}

#[cfg(test)]
mod tests {
    use super::is_employee;
    use serde_json::json;

    #[test]
    fn employee_classification_matches_gateway_domains_and_explicit_marks() {
        for email in ["Ada@Nexus.xyz", "Ada@Nexus.inc"] {
            assert!(is_employee(&json!({"user": {"email": email}})), "{email}");
        }
        for email in [
            "ada@nexus.xyz.evil.test",
            "ada@nexus.inc.evil.test",
            "ada@othernexus.inc",
            "ada@sub.nexus.inc",
            "ada@acme.test",
        ] {
            assert!(!is_employee(&json!({"user": {"email": email}})), "{email}");
        }
        assert!(is_employee(
            &json!({"user": {"email": "ada@acme.test", "mark": "staff"}})
        ));
        assert!(!is_employee(
            &json!({"user": {"email": "ada@acme.test", "mark": "test"}})
        ));
        assert!(!is_employee(
            &json!({"user": {"email": "ada@acme.test", "mark": "demo"}})
        ));
        assert!(!is_employee(&json!({"user": {}})));
    }
}
