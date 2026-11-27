use std::collections::HashMap;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{Dict, OwnedObjectPath, OwnedValue, Value};

use super::Secret;

const DESTINATION: &str = "org.freedesktop.secrets";
const ROOT: &str = "/org/freedesktop/secrets";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";

type Result<T> = std::result::Result<T, ()>;
type Attributes<'a> = HashMap<&'a str, &'a str>;
type WireSecret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

/// Only operations that can complete without a prompt are available to the store.
trait Bus {
    fn default_collection(&self) -> Result<OwnedObjectPath>;
    fn locked(&self, path: &OwnedObjectPath, interface: &str) -> Result<bool>;
    fn search(&self, collection: &OwnedObjectPath, attributes: Attributes<'_>) -> Result<Vec<OwnedObjectPath>>;
    fn open_session(&self) -> Result<OwnedObjectPath>;
    fn get(&self, item: &OwnedObjectPath, session: OwnedObjectPath) -> Result<Vec<u8>>;
    fn create(
        &self,
        collection: &OwnedObjectPath,
        session: OwnedObjectPath,
        attributes: Attributes<'_>,
        secret: &Secret,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath)>;
    fn delete(&self, item: &OwnedObjectPath) -> Result<OwnedObjectPath>;
}

struct SecretService(Connection);

impl SecretService {
    fn connect() -> Result<Self> {
        Connection::session().map(Self).map_err(|_| ())
    }

    fn proxy<'a>(&'a self, path: &'a str, interface: &'a str) -> Result<Proxy<'a>> {
        Proxy::new(&self.0, DESTINATION, path, interface).map_err(|_| ())
    }

    fn service(&self) -> Result<Proxy<'_>> {
        self.proxy(ROOT, "org.freedesktop.Secret.Service")
    }
}

impl Bus for SecretService {
    fn default_collection(&self) -> Result<OwnedObjectPath> {
        self.service()?.call("ReadAlias", &("default",)).map_err(|_| ())
    }

    fn locked(&self, path: &OwnedObjectPath, interface: &str) -> Result<bool> {
        self.proxy(path.as_str(), interface)?
            .get_property("Locked")
            .map_err(|_| ())
    }

    fn search(&self, collection: &OwnedObjectPath, attributes: Attributes<'_>) -> Result<Vec<OwnedObjectPath>> {
        self.proxy(collection.as_str(), COLLECTION)?
            .call("SearchItems", &(attributes,))
            .map_err(|_| ())
    }

    fn open_session(&self) -> Result<OwnedObjectPath> {
        // The local session-bus transport is scoped to this login.
        let (_, path): (OwnedValue, OwnedObjectPath) = self
            .service()?
            .call("OpenSession", &("plain", Value::from("")))
            .map_err(|_| ())?;
        Ok(path)
    }

    fn get(&self, item: &OwnedObjectPath, session: OwnedObjectPath) -> Result<Vec<u8>> {
        let (_, _, bytes, _): WireSecret = self
            .proxy(item.as_str(), ITEM)?
            .call("GetSecret", &(session,))
            .map_err(|_| ())?;
        Ok(bytes)
    }

    fn create(
        &self,
        collection: &OwnedObjectPath,
        session: OwnedObjectPath,
        attributes: Attributes<'_>,
        secret: &Secret,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath)> {
        let properties: HashMap<&str, Value<'_>> = HashMap::from([
            (
                "org.freedesktop.Secret.Item.Label",
                Value::from("Nexus Desktop provider key"),
            ),
            ("org.freedesktop.Secret.Item.Attributes", Dict::from(attributes).into()),
        ]);
        let wire = (session, &[] as &[u8], secret.expose().as_bytes(), "text/plain");
        self.proxy(collection.as_str(), COLLECTION)?
            .call("CreateItem", &(properties, wire, true))
            .map_err(|_| ())
    }

    fn delete(&self, item: &OwnedObjectPath) -> Result<OwnedObjectPath> {
        self.proxy(item.as_str(), ITEM)?.call("Delete", &()).map_err(|_| ())
    }
}

struct Store<'a, B> {
    bus: &'a B,
    collection: OwnedObjectPath,
}

impl<'a, B: Bus> Store<'a, B> {
    fn open(bus: &'a B) -> Result<Self> {
        let collection = bus.default_collection()?;
        if collection.as_str() == "/" || bus.locked(&collection, COLLECTION)? {
            return Err(());
        }
        Ok(Self { bus, collection })
    }

    fn item(&self, attributes: Attributes<'_>) -> Result<Option<OwnedObjectPath>> {
        let mut items = self.bus.search(&self.collection, attributes)?.into_iter();
        let Some(item) = items.next() else { return Ok(None) };
        if items.next().is_some() || self.bus.locked(&item, ITEM)? {
            return Err(());
        }
        Ok(Some(item))
    }

    fn read(&self, attributes: Attributes<'_>) -> Result<Option<Secret>> {
        let Some(item) = self.item(attributes)? else {
            return Ok(None);
        };
        let bytes = self.bus.get(&item, self.bus.open_session()?)?;
        String::from_utf8(bytes).map(Secret::new).map(Some).map_err(|_| ())
    }

    fn write(&self, attributes: Attributes<'_>, secret: &Secret) -> Result<()> {
        let session = self.bus.open_session()?;
        if self.bus.locked(&self.collection, COLLECTION)? {
            return Err(());
        }
        let (item, prompt) = self.bus.create(&self.collection, session, attributes, secret)?;
        if item.as_str() == "/" || prompt.as_str() != "/" {
            return Err(());
        }
        Ok(())
    }

    fn delete(&self, attributes: Attributes<'_>) -> Result<()> {
        if let Some(item) = self.item(attributes)? {
            let prompt = self.bus.delete(&item)?;
            if prompt.as_str() != "/" {
                return Err(());
            }
        }
        Ok(())
    }
}

fn attributes<'a>(service: &'a str, key: &'a str) -> Attributes<'a> {
    HashMap::from([("service", service), ("username", key)])
}

pub(super) fn read(service: &str, key: &str) -> Result<Option<Secret>> {
    Store::open(&SecretService::connect()?)?.read(attributes(service, key))
}

pub(super) fn write(service: &str, key: &str, secret: &Secret) -> Result<()> {
    Store::open(&SecretService::connect()?)?.write(attributes(service, key), secret)
}

pub(super) fn delete(service: &str, key: &str) -> Result<()> {
    Store::open(&SecretService::connect()?)?.delete(attributes(service, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeBus {
        missing: bool,
        locked: bool,
        item_locked: bool,
        prompt: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    fn path(value: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(value).expect("test object path")
    }

    impl FakeBus {
        fn record(&self, method: &'static str) {
            self.calls.borrow_mut().push(method);
        }

        fn prompt_path(&self) -> OwnedObjectPath {
            path(if self.prompt { "/prompt" } else { "/" })
        }
    }

    impl Bus for FakeBus {
        fn default_collection(&self) -> Result<OwnedObjectPath> {
            self.record("ReadAlias");
            Ok(path(if self.missing { "/" } else { "/collection" }))
        }

        fn locked(&self, _: &OwnedObjectPath, interface: &str) -> Result<bool> {
            self.record("Locked");
            Ok(if interface == ITEM {
                self.item_locked
            } else {
                self.locked
            })
        }

        fn search(&self, _: &OwnedObjectPath, _: Attributes<'_>) -> Result<Vec<OwnedObjectPath>> {
            self.record("SearchItems");
            Ok(vec![path("/item")])
        }

        fn open_session(&self) -> Result<OwnedObjectPath> {
            self.record("OpenSession");
            Ok(path("/session"))
        }

        fn get(&self, _: &OwnedObjectPath, _: OwnedObjectPath) -> Result<Vec<u8>> {
            self.record("GetSecret");
            Ok(b"fake-key".to_vec())
        }

        fn create(
            &self,
            _: &OwnedObjectPath,
            _: OwnedObjectPath,
            _: Attributes<'_>,
            _: &Secret,
        ) -> Result<(OwnedObjectPath, OwnedObjectPath)> {
            self.record("CreateItem");
            Ok((path("/item"), self.prompt_path()))
        }

        fn delete(&self, _: &OwnedObjectPath) -> Result<OwnedObjectPath> {
            self.record("Delete");
            Ok(self.prompt_path())
        }
    }

    #[test]
    fn locked_or_missing_default_collection_fails_without_any_prompt() {
        for (bus, expected) in [
            (
                FakeBus {
                    missing: true,
                    ..FakeBus::default()
                },
                &["ReadAlias"][..],
            ),
            (
                FakeBus {
                    locked: true,
                    ..FakeBus::default()
                },
                &["ReadAlias", "Locked"][..],
            ),
        ] {
            assert!(Store::open(&bus).is_err());
            assert_eq!(*bus.calls.borrow(), expected);
        }
    }

    #[test]
    fn locked_item_is_not_read_or_deleted() {
        let bus = FakeBus {
            item_locked: true,
            ..FakeBus::default()
        };
        let store = Store::open(&bus).expect("unlocked collection");
        assert!(store.read(attributes("test", "key")).is_err());
        assert!(store.delete(attributes("test", "key")).is_err());
        assert_eq!(
            *bus.calls.borrow(),
            ["ReadAlias", "Locked", "SearchItems", "Locked", "SearchItems", "Locked"]
        );
    }

    #[test]
    fn create_and_delete_fail_when_the_service_requires_a_prompt() {
        let bus = FakeBus {
            prompt: true,
            ..FakeBus::default()
        };
        let store = Store::open(&bus).expect("unlocked collection");
        assert!(
            store
                .write(attributes("test", "key"), &Secret::new("fake-key".into()))
                .is_err()
        );
        assert!(store.delete(attributes("test", "key")).is_err());
        assert_eq!(
            *bus.calls.borrow(),
            [
                "ReadAlias",
                "Locked",
                "OpenSession",
                "Locked",
                "CreateItem",
                "SearchItems",
                "Locked",
                "Delete",
            ]
        );
    }
}
