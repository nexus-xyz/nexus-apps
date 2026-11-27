//! Local provider settings and the credential references they own.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use nexus_providers::{
    ApiKeySetting, InstanceId, KeyRef, KeySource, KeyVendor, ProviderError, ProviderSettings, Secret, ServerSetting,
};

use crate::atomic_file::write_atomic_private;

#[derive(Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    #[serde(flatten)]
    providers: ProviderSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending_key_deletes: Vec<KeyRef>,
}

impl SettingsFile {
    fn retire(&mut self, key: Option<KeyRef>) {
        if let Some(key) = key
            && !referenced(&self.providers, &key)
            && !self.pending_key_deletes.contains(&key)
        {
            self.pending_key_deletes.push(key);
        }
    }
}

/// The metadata displayed before a person confirmed an edit.
pub enum ExpectedProvider<'a> {
    Key(&'a ApiKeySetting),
    Server(&'a ServerSetting),
}

impl ExpectedProvider<'_> {
    fn id(&self) -> &InstanceId {
        match self {
            Self::Key(key) => &key.id,
            Self::Server(server) => &server.id,
        }
    }

    fn matches(&self, settings: &ProviderSettings) -> bool {
        match self {
            Self::Key(key) => settings.api_keys.contains(key),
            Self::Server(server) => settings.servers.contains(server),
        }
    }
}

fn changed_provider() -> ProviderError {
    ProviderError::Refused("This provider changed. Open it again before saving edits.".into())
}

pub struct ProviderSettingsStore {
    file: PathBuf,
    #[cfg(test)]
    after_read: Option<Box<dyn Fn() + Send + Sync>>,
}

/// Read the app's current saved providers in the process that will launch the agent.
pub async fn read_for_launch() -> Result<ProviderSettings, String> {
    tokio::task::spawn_blocking(read_saved_providers)
        .await
        .map_err(|_| "Nexus could not read the provider settings.".to_string())?
}

fn read_saved_providers() -> Result<ProviderSettings, String> {
    let dir = crate::settings::config_dir().ok_or("This computer has no app configuration folder.")?;
    ProviderSettingsStore::in_config(&dir)
        .read()
        .map_err(|error| error.to_string())
}

/// Look up the server's current models for an explicit change to a saved session.
pub fn saved_server_models(provider: &InstanceId) -> Result<Vec<String>, String> {
    read_saved_providers()?
        .servers
        .into_iter()
        .find(|server| &server.id == provider)
        .map(|server| server.models)
        .ok_or_else(|| "This server was removed. Check your saved servers in Settings.".into())
}

impl ProviderSettingsStore {
    pub fn in_config(dir: &Path) -> Self {
        Self {
            file: dir.join("providers.json"),
            #[cfg(test)]
            after_read: None,
        }
    }

    pub fn read(&self) -> Result<ProviderSettings, ProviderError> {
        Ok(self.read_file()?.providers)
    }

    /// Read displayable settings and whether unused credentials still need deletion.
    pub fn read_with_cleanup_status(&self) -> Result<(ProviderSettings, bool), ProviderError> {
        let file = self.read_file()?;
        Ok((file.providers, !file.pending_key_deletes.is_empty()))
    }

    fn read_file(&self) -> Result<SettingsFile, ProviderError> {
        match std::fs::read(&self.file) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| {
                ProviderError::Refused("Nexus could not read the provider settings. Check providers.json.".into())
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(SettingsFile::default()),
            Err(error) => Err(settings_io(error)),
        }
    }

    fn edit<T>(&self, update: impl FnOnce(&mut SettingsFile) -> Result<T, ProviderError>) -> Result<T, ProviderError> {
        self.edit_file(|settings| {
            #[cfg(test)]
            if let Some(after_read) = &self.after_read {
                after_read();
            }
            update(settings)
        })
    }

    fn edit_file<T>(
        &self,
        update: impl FnOnce(&mut SettingsFile) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let _lock = lock_provider_settings(&self.file).map_err(settings_io)?;
        let mut settings = self.read_file()?;
        let result = update(&mut settings)?;
        let bytes = serde_json::to_vec_pretty(&settings)
            .map_err(io::Error::other)
            .map_err(settings_io)?;
        write_atomic_private(&self.file, &bytes).map_err(settings_io)?;
        Ok(result)
    }

    pub fn add_api_key(
        &self,
        keys: &dyn KeySource,
        vendor: KeyVendor,
        secret: Secret,
    ) -> Result<InstanceId, ProviderError> {
        let id = new_instance("key");
        self.stage_key(keys, secret, |settings, key| {
            settings.providers.api_keys.push(ApiKeySetting {
                id: id.clone(),
                vendor,
                key,
            });
            Ok(())
        })?;
        Ok(id)
    }

    pub fn add_server(
        &self,
        name: String,
        base_url: &str,
        models: Vec<String>,
        credential: Option<(&dyn KeySource, Secret)>,
    ) -> Result<InstanceId, ProviderError> {
        let id = new_instance("server");
        let base_url = nexus_workbench::provider::checked_server_base_url(base_url, credential.is_some())?;
        let save = |settings: &mut SettingsFile, key| {
            settings.providers.servers.push(ServerSetting {
                id: id.clone(),
                name,
                base_url,
                key,
                models,
            });
            Ok(())
        };
        match credential {
            Some((keys, secret)) => self.stage_key(keys, secret, |settings, key| save(settings, Some(key)))?,
            None => self.edit(|settings| save(settings, None))?,
        }
        Ok(id)
    }

    pub fn edit_server(
        &self,
        id: &InstanceId,
        name: String,
        base_url: &str,
        models: Vec<String>,
    ) -> Result<(), ProviderError> {
        self.edit_server_at(id, None, name, base_url, models)
    }

    /// Save changes only if the row still matches the one the editor opened.
    pub fn edit_server_if_current(
        &self,
        expected: &ServerSetting,
        name: String,
        base_url: &str,
        models: Vec<String>,
    ) -> Result<(), ProviderError> {
        self.edit_server_at(&expected.id, Some(expected), name, base_url, models)
    }

    fn edit_server_at(
        &self,
        id: &InstanceId,
        expected: Option<&ServerSetting>,
        name: String,
        base_url: &str,
        models: Vec<String>,
    ) -> Result<(), ProviderError> {
        let base_url = nexus_workbench::provider::server_base_url(base_url)
            .ok_or_else(|| ProviderError::BadBaseUrl("This address".into()))?;
        self.edit(|settings| {
            let server = settings
                .providers
                .servers
                .iter_mut()
                .find(|server| &server.id == id)
                .ok_or_else(missing_provider)?;
            if expected.is_some_and(|expected| expected != server) {
                return Err(ProviderError::Refused(
                    "This server changed. Open it again before saving edits.".into(),
                ));
            }
            if server.key.is_some() && server.base_url != base_url {
                return Err(ProviderError::Refused(
                    "Remove the server's key before changing its address.".into(),
                ));
            }
            server.name = name;
            server.base_url = base_url;
            server.models = models;
            Ok(())
        })
    }

    /// Record the replacement before writing its credential.
    pub fn rotate_api_key(&self, keys: &dyn KeySource, id: &InstanceId, secret: Secret) -> Result<(), ProviderError> {
        self.rotate_api_key_at(keys, id, None, secret)
    }

    pub fn rotate_api_key_if_current(
        &self,
        keys: &dyn KeySource,
        expected: &ApiKeySetting,
        secret: Secret,
    ) -> Result<(), ProviderError> {
        self.rotate_api_key_at(keys, &expected.id, Some(expected), secret)
    }

    fn rotate_api_key_at(
        &self,
        keys: &dyn KeySource,
        id: &InstanceId,
        expected: Option<&ApiKeySetting>,
        secret: Secret,
    ) -> Result<(), ProviderError> {
        self.stage_key(keys, secret, |settings, fresh| {
            let setting = settings
                .providers
                .api_keys
                .iter_mut()
                .find(|setting| &setting.id == id)
                .ok_or_else(missing_provider)?;
            if expected.is_some_and(|expected| expected != setting) {
                return Err(changed_provider());
            }
            let old = std::mem::replace(&mut setting.key, fresh);
            settings.retire(Some(old));
            Ok(())
        })
    }

    /// Bind the staged key to the server address the person saw when entering it.
    pub fn rotate_server_key(
        &self,
        keys: &dyn KeySource,
        id: &InstanceId,
        expected_address: &str,
        secret: Secret,
    ) -> Result<(), ProviderError> {
        self.rotate_server_key_at(keys, id, expected_address, None, secret)
    }

    pub fn rotate_server_key_if_current(
        &self,
        keys: &dyn KeySource,
        expected: &ServerSetting,
        secret: Secret,
    ) -> Result<(), ProviderError> {
        self.rotate_server_key_at(keys, &expected.id, &expected.base_url, Some(expected), secret)
    }

    fn rotate_server_key_at(
        &self,
        keys: &dyn KeySource,
        id: &InstanceId,
        expected_address: &str,
        expected: Option<&ServerSetting>,
        secret: Secret,
    ) -> Result<(), ProviderError> {
        let expected_address = nexus_workbench::provider::checked_server_base_url(expected_address, true)?;
        self.stage_key(keys, secret, |settings, fresh| {
            let server = settings
                .providers
                .servers
                .iter_mut()
                .find(|setting| &setting.id == id)
                .ok_or_else(missing_provider)?;
            if expected_address != server.base_url {
                return Err(ProviderError::Refused(
                    "The server address changed. Check the address before saving its key.".into(),
                ));
            }
            if expected.is_some_and(|expected| expected != server) {
                return Err(changed_provider());
            }
            let old = server.key.replace(fresh);
            settings.retire(old);
            Ok(())
        })
    }

    pub fn clear_server_key(&self, keys: &dyn KeySource, id: &InstanceId) -> Result<(), ProviderError> {
        self.clear_server_key_at(keys, id, None)
    }

    pub fn clear_server_key_if_current(
        &self,
        keys: &dyn KeySource,
        expected: &ServerSetting,
    ) -> Result<(), ProviderError> {
        self.clear_server_key_at(keys, &expected.id, Some(expected))
    }

    fn clear_server_key_at(
        &self,
        keys: &dyn KeySource,
        id: &InstanceId,
        expected: Option<&ServerSetting>,
    ) -> Result<(), ProviderError> {
        self.edit(|settings| {
            let server = settings
                .providers
                .servers
                .iter_mut()
                .find(|server| &server.id == id)
                .ok_or_else(missing_provider)?;
            if expected.is_some_and(|expected| expected != server) {
                return Err(changed_provider());
            }
            let old = server.key.take();
            settings.retire(old);
            Ok(())
        })?;
        self.finish_key_edit(keys)
    }

    pub fn remove(&self, keys: &dyn KeySource, id: &InstanceId) -> Result<(), ProviderError> {
        self.remove_at(keys, id, None)
    }

    pub fn remove_if_current(&self, keys: &dyn KeySource, expected: ExpectedProvider<'_>) -> Result<(), ProviderError> {
        self.remove_at(keys, expected.id(), Some(&expected))
    }

    fn remove_at(
        &self,
        keys: &dyn KeySource,
        id: &InstanceId,
        expected: Option<&ExpectedProvider<'_>>,
    ) -> Result<(), ProviderError> {
        self.edit(|settings| {
            if expected.is_some_and(|expected| !expected.matches(&settings.providers)) {
                return Err(changed_provider());
            }
            let old = if let Some(at) = settings.providers.api_keys.iter().position(|setting| &setting.id == id) {
                Some(settings.providers.api_keys.remove(at).key)
            } else if let Some(at) = settings.providers.servers.iter().position(|setting| &setting.id == id) {
                settings.providers.servers.remove(at).key
            } else {
                return Err(missing_provider());
            };
            if settings.providers.default.as_ref() == Some(id) {
                settings.providers.default = None;
            }
            settings.retire(old);
            Ok(())
        })?;
        self.finish_key_edit(keys)
    }

    fn stage_key<T>(
        &self,
        keys: &dyn KeySource,
        secret: Secret,
        publish: impl FnOnce(&mut SettingsFile, KeyRef) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let id = uuid::Uuid::new_v4();
        let key = KeyRef::new(format!("provider-key/{id}"));
        let path = self.key_operation_path(id)?;
        let file = open_private_lock(&path).map_err(settings_io)?;
        file.lock().map_err(settings_io)?;
        let guard = SettingsLock(file);
        // Record the reference before any keychain call. Recovery skips it while this writer holds the guard.
        self.edit_file(|settings| {
            settings.pending_key_deletes.push(key.clone());
            Ok(())
        })?;
        let result = keys.write(&key, secret).and_then(|()| {
            self.edit(|settings| {
                let result = publish(settings, key.clone())?;
                settings.pending_key_deletes.retain(|pending| pending != &key);
                Ok(result)
            })
        });
        drop(guard);
        if result.is_ok() {
            let _ = std::fs::remove_file(path);
        }
        // A failed delete stays in the file. Keep the result of the requested edit.
        let _ = self.retry_key_cleanup(keys);
        result
    }

    /// Retry abandoned and retired credentials. Call off the UI thread when the credential store opens.
    pub fn retry_key_cleanup(&self, keys: &dyn KeySource) -> Result<(), ProviderError> {
        let mut failure = Ok(());
        for key in self.read_file()?.pending_key_deletes {
            if let Err(error) = self.retry_delete(keys, &key) {
                failure = Err(error);
            }
        }
        failure
    }

    fn finish_key_edit(&self, keys: &dyn KeySource) -> Result<(), ProviderError> {
        self.retry_key_cleanup(keys).map_err(|_| {
            ProviderError::Refused(
                "Provider settings saved. Nexus will retry removing unused keys from the keychain.".into(),
            )
        })
    }

    fn retry_delete(&self, keys: &dyn KeySource, key: &KeyRef) -> Result<(), ProviderError> {
        let id = key
            .as_str()
            .strip_prefix("provider-key/")
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .ok_or_else(|| ProviderError::Refused("This provider has an invalid key reference.".into()))?;
        let path = self.key_operation_path(id)?;
        let file = open_private_lock(&path).map_err(settings_io)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
            Err(std::fs::TryLockError::Error(error)) => return Err(settings_io(error)),
        }
        let guard = SettingsLock(file);
        let state = self.read_file()?;
        if state.pending_key_deletes.contains(key) {
            if !referenced(&state.providers, key) {
                keys.delete(key)?;
            }
            self.edit_file(|settings| {
                settings.pending_key_deletes.retain(|pending| pending != key);
                Ok(())
            })?;
        }
        drop(guard);
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    fn key_operation_path(&self, id: uuid::Uuid) -> Result<PathBuf, ProviderError> {
        let dir = self
            .file
            .parent()
            .ok_or_else(|| settings_io(io::ErrorKind::InvalidInput.into()))?;
        std::fs::create_dir_all(dir).map_err(settings_io)?;
        Ok(dir.join(format!(".provider-key-{id}.lock")))
    }
}

fn settings_io(error: io::Error) -> ProviderError {
    ProviderError::Refused(format!(
        "Nexus could not open or save provider settings ({:?}).",
        error.kind()
    ))
}

fn missing_provider() -> ProviderError {
    ProviderError::Refused("This provider no longer exists. Select a provider in Settings.".into())
}

fn new_instance(kind: &str) -> InstanceId {
    InstanceId::new(format!("{kind}:{}", uuid::Uuid::new_v4()))
}

fn referenced(settings: &ProviderSettings, key: &KeyRef) -> bool {
    settings.api_keys.iter().any(|setting| &setting.key == key)
        || settings.servers.iter().any(|setting| setting.key.as_ref() == Some(key))
}

fn open_private_lock(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

struct SettingsLock(File);

impl Drop for SettingsLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// The OS releases the lock when the holder dies. Reopen the same file on each transaction.
/// The file can remain after exit and never needs deletion or a PID check.
fn lock_provider_settings(file: &Path) -> io::Result<SettingsLock> {
    let dir = file.parent().ok_or(io::ErrorKind::InvalidInput)?;
    std::fs::create_dir_all(dir)?;
    let lock = open_private_lock(&dir.join("providers.json.lock"))?;
    lock.lock()?;
    Ok(SettingsLock(lock))
}

#[cfg(test)]
#[path = "provider_settings_tests.rs"]
mod tests;
