# nexus-providers

`nexus-providers` holds the plain values of Nexus's provider layer. A provider is how a coding agent's session is
reached and paid: the person's plan, their own API key, a model server, or Nexus Cloud.

The crate has the provider kinds, how a model is funded, model offers, credentials, provider settings and errors.
It does no I/O. The app reads the credential store and the network.

## Usage

```rust
use nexus_providers::ProviderKind;

let kind = ProviderKind::parse("api_key").expect("a known kind");
assert_eq!(kind, ProviderKind::ApiKey);
assert!(!kind.entry().needs_nexus_account);

for kind in ProviderKind::ALL {
    println!("{kind}: {}", kind.entry().name);
}
```

`Secret` holds a key. It is wiped on drop, and it is never printed or serialized.

## License

MIT.
