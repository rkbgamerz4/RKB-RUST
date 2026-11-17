//! Refresh-token storage.
//!
//! A Minecraft client can stay signed in by keeping the OAuth refresh token.
//! That token is a credential: anyone holding it can obtain new access tokens.
//! The defaults here are therefore conservative.
//!
//! - [`MemoryTokenStore`] is the default and writes nothing to disk.
//! - [`FileTokenStore`] is explicit opt-in. It creates the file with mode
//!   `0600`, refuses to read a file that another user can access, refuses
//!   symbolic links, and replaces the file atomically.
//!
//! The stored document contains only the refresh token, the Java profile UUID,
//! and the profile name. Access tokens are never written. `Debug` for every
//! type here redacts the token.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use zeroize::Zeroize;

use crate::error::AuthError;
use crate::json::{take_required_secret, take_required_string, take_u64};
use crate::minecraft::uuid_hex;
use crate::secret::{SecretString, ZeroizingJson};

/// The document version written by [`FileTokenStore`].
const STORE_VERSION: u64 = 1;

/// One stored sign-in: the refresh token plus the profile it belongs to.
pub struct StoredSession {
    refresh_token: SecretString,
    uuid: u128,
    name: String,
}

impl StoredSession {
    /// Builds a stored session.
    #[must_use]
    pub fn new(refresh_token: SecretString, uuid: u128, name: impl Into<String>) -> Self {
        Self {
            refresh_token,
            uuid,
            name: name.into(),
        }
    }

    /// The refresh token. The value stays an opaque secret because the raw
    /// string accessor is crate-private.
    #[must_use]
    pub fn refresh_token(&self) -> &SecretString {
        &self.refresh_token
    }

    /// The Java profile UUID this token was issued for.
    #[must_use]
    pub fn uuid(&self) -> u128 {
        self.uuid
    }

    /// The Java profile name this token was issued for.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for StoredSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredSession")
            .field("refresh_token", &self.refresh_token)
            .field("uuid", &uuid_hex(self.uuid))
            .field("name", &self.name)
            .finish()
    }
}

/// A place to keep one refresh token.
pub trait TokenStore {
    /// Loads a stored session, if there is one.
    fn load(&self) -> Result<Option<StoredSession>, AuthError>;

    /// Replaces the stored session.
    fn save(&self, session: &StoredSession) -> Result<(), AuthError>;

    /// Removes the stored session. Removing a missing session succeeds.
    fn clear(&self) -> Result<(), AuthError>;
}

/// An in-memory store. This is the default; it survives only for the lifetime
/// of the process.
#[derive(Default)]
pub struct MemoryTokenStore {
    session: Mutex<Option<StoredSession>>,
}

impl MemoryTokenStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl fmt::Debug for MemoryTokenStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MemoryTokenStore")
            .field(
                "has_session",
                &self.session.lock().map(|s| s.is_some()).unwrap_or(false),
            )
            .finish()
    }
}

impl TokenStore for MemoryTokenStore {
    fn load(&self) -> Result<Option<StoredSession>, AuthError> {
        let guard = self
            .session
            .lock()
            .map_err(|_| AuthError::storage("the in-memory token lock is poisoned"))?;
        Ok(guard.as_ref().map(|session| {
            StoredSession::new(
                SecretString::new(session.refresh_token.expose()),
                session.uuid,
                session.name.clone(),
            )
        }))
    }

    fn save(&self, session: &StoredSession) -> Result<(), AuthError> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| AuthError::storage("the in-memory token lock is poisoned"))?;
        *guard = Some(StoredSession::new(
            SecretString::new(session.refresh_token.expose()),
            session.uuid,
            session.name.clone(),
        ));
        Ok(())
    }

    fn clear(&self) -> Result<(), AuthError> {
        let mut guard = self
            .session
            .lock()
            .map_err(|_| AuthError::storage("the in-memory token lock is poisoned"))?;
        *guard = None;
        Ok(())
    }
}

/// A refresh token in one JSON file with owner-only permissions.
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    /// A store at `path`. The parent directory must already exist and should be
    /// private to the user.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The configured path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Debug for FileTokenStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileTokenStore")
            .field("path", &self.path)
            .finish()
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> Result<Option<StoredSession>, AuthError> {
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(AuthError::storage(format!(
                    "could not inspect the token file: {error}"
                )))
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(AuthError::storage(
                "refusing to read a symbolic link as a token file",
            ));
        }
        if !metadata.file_type().is_file() {
            return Err(AuthError::storage("the token path is not a regular file"));
        }
        check_private_permissions(&metadata)?;

        let mut bytes = std::fs::read(&self.path).map_err(|error| {
            AuthError::storage(format!("could not read the token file: {error}"))
        })?;
        let mut value = ZeroizingJson::parse(&mut bytes).map_err(|error| {
            AuthError::storage(format!("the token file is not valid JSON: {error}"))
        })?;

        let version = take_u64(&mut value, "version")
            .ok_or_else(|| AuthError::storage("the token file has no version"))?;
        if version != STORE_VERSION {
            return Err(AuthError::storage(format!(
                "unsupported token file version {version}; expected {STORE_VERSION}"
            )));
        }
        let refresh_token = take_required_secret(&mut value, "refresh_token")
            .map_err(|_| AuthError::storage("the token file has no refresh_token"))?;
        if refresh_token.is_empty() {
            return Err(AuthError::storage("the stored refresh token is empty"));
        }
        let uuid_text = take_required_string(&mut value, "uuid")
            .map_err(|_| AuthError::storage("the token file has no uuid"))?;
        let uuid = crate::minecraft::parse_uuid_hex(&uuid_text)
            .map_err(|_| AuthError::storage("the token file has an invalid uuid"))?;
        let name = take_required_string(&mut value, "name")
            .map_err(|_| AuthError::storage("the token file has no name"))?;

        Ok(Some(StoredSession::new(refresh_token, uuid, name)))
    }

    fn save(&self, session: &StoredSession) -> Result<(), AuthError> {
        let value = ZeroizingJson::new(json!({
            "version": STORE_VERSION,
            "refresh_token": session.refresh_token.expose(),
            "uuid": uuid_hex(session.uuid),
            "name": session.name,
        }));
        let mut body = value
            .serialize()
            .map_err(|error| {
                AuthError::storage(format!("could not encode the token file: {error}"))
            })?
            .into_bytes();
        let result = write_private_atomically(&self.path, &body);
        body.zeroize();
        result
    }

    fn clear(&self) -> Result<(), AuthError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AuthError::storage(format!(
                "could not remove the token file: {error}"
            ))),
        }
    }
}

#[cfg(unix)]
fn check_private_permissions(metadata: &std::fs::Metadata) -> Result<(), AuthError> {
    use std::os::unix::fs::PermissionsExt;

    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(AuthError::storage(format!(
            "the token file mode is {mode:03o}; it must not be readable or writable by \
             group or other users (use chmod 600)"
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_private_permissions(_metadata: &std::fs::Metadata) -> Result<(), AuthError> {
    Ok(())
}

fn write_private_atomically(path: &Path, body: &[u8]) -> Result<(), AuthError> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if !parent.is_dir() {
        return Err(AuthError::storage(
            "the token file directory does not exist; create it first",
        ));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("token");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(".{name}.{}.{nanos}.tmp", std::process::id()));

    let result = write_new_file(&temporary, body).and_then(|()| {
        replace_file(&temporary, path).map_err(|error| {
            AuthError::storage(format!("could not replace the token file: {error}"))
        })
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn write_new_file(path: &Path, body: &[u8]) -> Result<(), AuthError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| AuthError::storage(format!("could not create the token file: {error}")))?;
    file.write_all(body)
        .and_then(|()| file.sync_all())
        .map_err(|error| AuthError::storage(format!("could not write the token file: {error}")))
}

#[cfg(unix)]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(temporary, destination)
}

#[cfg(not(unix))]
fn replace_file(temporary: &Path, destination: &Path) -> std::io::Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)?;
    }
    std::fs::rename(temporary, destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temporary_path(name: &str) -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "rkb-auth-{name}-{}-{unique}.json",
            std::process::id()
        ))
    }

    fn session() -> StoredSession {
        StoredSession::new(SecretString::new("refresh-secret"), 7, "Notch")
    }

    #[test]
    fn memory_store_round_trips_and_clears() {
        let store = MemoryTokenStore::new();
        assert!(store.load().unwrap().is_none());
        store.save(&session()).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.uuid(), 7);
        assert_eq!(loaded.name(), "Notch");
        assert_eq!(loaded.refresh_token().expose(), "refresh-secret");
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn debug_redacts_the_refresh_token() {
        assert!(!format!("{:?}", session()).contains("refresh-secret"));
        let store = MemoryTokenStore::new();
        store.save(&session()).unwrap();
        assert!(!format!("{store:?}").contains("refresh-secret"));
    }

    #[test]
    fn file_store_round_trips_with_owner_only_mode() {
        let path = temporary_path("roundtrip");
        let store = FileTokenStore::new(&path);
        assert!(store.load().unwrap().is_none());
        store.save(&session()).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.is_empty());
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("\"refresh_token\":\"refresh-secret\""));

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.name(), "Notch");
        assert_eq!(loaded.uuid(), 7);
        assert_eq!(loaded.refresh_token().expose(), "refresh-secret");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "unexpected mode {mode:03o}");
        }

        store.clear().unwrap();
        assert!(!path.exists());
        store.clear().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_store_rejects_group_or_other_access() {
        use std::os::unix::fs::PermissionsExt;

        let path = temporary_path("permissions");
        let store = FileTokenStore::new(&path);
        store.save(&session()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = store.load().unwrap_err();
        assert!(error.to_string().contains("0600") || error.to_string().contains("644"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_store_rejects_a_missing_directory() {
        let path = temporary_path("missing").join("token.json");
        let store = FileTokenStore::new(path);
        assert!(store.save(&session()).is_err());
    }

    #[test]
    fn file_store_rejects_unknown_versions() {
        let path = temporary_path("version");
        std::fs::write(
            &path,
            r#"{"version":99,"refresh_token":"t","uuid":"00000000000000000000000000000001","name":"N"}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let store = FileTokenStore::new(&path);
        assert!(store.load().is_err());
        let _ = std::fs::remove_file(&path);
    }
}
