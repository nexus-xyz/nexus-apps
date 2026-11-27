// Test cases for semgrep.yml. Run: semgrep --test --config .github/oss-leak-gate/semgrep.yml .github/oss-leak-gate/
// Every value here is synthetic.

// ruleid: rust-credential-literal
const API_KEY: &str = "zq9X7a1B2c3D4e5F6g7H8i9J";

fn f() {
    // ruleid: rust-credential-literal
    let client_secret = "Q2xpZW50U2VjcmV0MTIzNDU2Nzg5";
    // ruleid: rust-credential-literal
    let token: &str = "ghx_0aB1cD2eF3gH4iJ5kL6mN7oP8";

    // ok: rust-credential-literal
    let fake_token = "zq9X7a1B2c3D4e5F6g7H8i9J";
    // ok: rust-credential-literal
    let token = "short1";
}

// A header's name, not a value.
// ok: rust-credential-literal
pub const TOKEN_HEADER: &str = "x-nexus-session-token-v2";
// An env var's name, not a value.
// ok: rust-credential-literal
pub const CLAUDE_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN_V2";
// No digits: a word, not a credential.
// ok: rust-credential-literal
static PASSWORD: &str = "correcthorsebatterystaple";
