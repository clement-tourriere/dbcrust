//! Encrypted secret file (`secrets.enc`): AES-256-GCM records under a key
//! derived from a passphrase (Argon2id) or read from a strict key file.
//!
//! Layout (all integers big-endian):
//!
//! ```text
//! magic "DBCSECR1" | version u16 | kdf u8 | m_kib u32 | t u32 | p u32 | salt[16]
//! | check_nonce[12] | check_ct[37]            (AAD = everything above)
//! | record_count u32
//! | records: id_len u16 | id | nonce[12] | ct_len u32 | ct
//! ```
//!
//! Each record is sealed with a fresh random 96-bit nonce and AAD =
//! `version u16 ‖ record id`, so a ciphertext moved to another id fails to
//! open. The `check` block proves a key is right without touching a record,
//! which is how a wrong passphrase is told apart from a corrupt record.
//! Record ids are stored in the clear (the same ids are in the non-secret
//! index); values never are.

use super::fsutil::{self, FileLock};
use super::{BackendKind, BackendState, BackendStatus, SecretBackend, SecretError};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, Payload, rand_core::RngCore},
};
use argon2::{Algorithm, Argon2, Params, Version};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

/// File name of the encrypted store inside the DBCrust config directory.
pub const SECRETS_FILE_NAME: &str = "secrets.enc";

const MAGIC: &[u8; 8] = b"DBCSECR1";
const FORMAT_VERSION: u16 = 1;
const KDF_NONE: u8 = 0;
const KDF_ARGON2ID: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const CHECK_PLAINTEXT: &[u8] = b"dbcrust-secrets-check";
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ID_LEN: usize = 512;
const MAX_RECORD_CT: usize = 1024 * 1024;
const MAX_RECORDS: u32 = 100_000;
const MIN_PASSPHRASE_CHARS: usize = 8;

type Key = Zeroizing<[u8; 32]>;

/// Argon2id cost parameters, stored in the file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 1 lane.
    pub const DEFAULT: KdfParams = KdfParams {
        m_cost_kib: 64 * 1024,
        t_cost: 3,
        p_cost: 1,
    };

    /// Deliberately tiny parameters so tests stay fast. Never use in
    /// production: they offer no brute-force resistance.
    pub const INSECURE_FOR_TESTS: KdfParams = KdfParams {
        m_cost_kib: 8,
        t_cost: 1,
        p_cost: 1,
    };

    /// Bounds a header must respect: refuses values that would be unusable
    /// or let a corrupt/hostile file demand gigabytes of memory.
    fn validate(&self) -> Result<(), SecretError> {
        let ok = (8..=1_048_576).contains(&self.m_cost_kib)
            && (1..=64).contains(&self.t_cost)
            && (1..=16).contains(&self.p_cost)
            && self.m_cost_kib >= 8 * self.p_cost;
        if ok {
            Ok(())
        } else {
            Err(SecretError::Corrupt(
                "key derivation parameters are out of range".to_string(),
            ))
        }
    }
}

/// Source of the passphrase for the encrypted file. Implementations must
/// never read it from argv, the environment or disk.
pub trait PassphraseProvider: Send + Sync {
    /// Obtain the passphrase. `confirm` is true when a *new* passphrase is
    /// being chosen (ask twice, enforce a minimum length).
    fn passphrase(&self, confirm: bool) -> Result<Zeroizing<String>, SecretError>;

    /// Whether a wrong passphrase may be retried (interactive prompts).
    fn can_retry(&self) -> bool {
        false
    }
}

/// Prompts on the terminal with `inquire::Password`. Returns
/// [`SecretError::Locked`] when prompts are not allowed (non-interactive).
#[derive(Debug, Clone, Copy, Default)]
pub struct PromptPassphrase;

impl PassphraseProvider for PromptPassphrase {
    fn passphrase(&self, confirm: bool) -> Result<Zeroizing<String>, SecretError> {
        if !super::is_interactive() {
            return Err(SecretError::Locked(
                "a passphrase is needed but no interactive terminal is available".to_string(),
            ));
        }
        let prompt = if confirm {
            inquire::Password::new("Choose a passphrase for DBCrust secrets:")
                .with_custom_confirmation_message("Repeat the passphrase:")
                .with_custom_confirmation_error_message("The passphrases do not match.")
                .with_validator(|input: &str| {
                    if input.chars().count() >= MIN_PASSPHRASE_CHARS {
                        Ok(inquire::validator::Validation::Valid)
                    } else {
                        Ok(inquire::validator::Validation::Invalid(
                            format!("Use at least {MIN_PASSPHRASE_CHARS} characters.").into(),
                        ))
                    }
                })
                .with_help_message(
                    "Encrypts the file holding your saved database passwords. There is no recovery if you forget it.",
                )
        } else {
            inquire::Password::new("DBCrust secrets passphrase:").without_confirmation()
        };
        prompt
            .prompt()
            .map(Zeroizing::new)
            .map_err(|e| SecretError::Locked(format!("passphrase entry was cancelled: {e}")))
    }

    fn can_retry(&self) -> bool {
        true
    }
}

/// A passphrase already held in memory (broker unlock, tests).
pub struct FixedPassphrase(Zeroizing<String>);

impl FixedPassphrase {
    pub fn new(passphrase: &str) -> Self {
        Self(Zeroizing::new(passphrase.to_string()))
    }
}

impl PassphraseProvider for FixedPassphrase {
    fn passphrase(&self, _confirm: bool) -> Result<Zeroizing<String>, SecretError> {
        Ok(self.0.clone())
    }
}

/// Where the 256-bit file key comes from.
#[derive(Clone)]
pub enum KeySource {
    /// Argon2id over a passphrase (interactive store).
    Passphrase(Arc<dyn PassphraseProvider>),
    /// 32 random bytes in a 0600 file in a 0700 directory, both owned by the
    /// current user (service-mode broker). `create` makes it when missing.
    KeyFile { path: PathBuf, create: bool },
    /// A raw key already in memory.
    Key(Zeroizing<[u8; 32]>),
}

impl KeySource {
    /// Prompt the user for a passphrase with `inquire`.
    pub fn prompt() -> Self {
        KeySource::Passphrase(Arc::new(PromptPassphrase))
    }

    /// Use (and, with `create`, generate) a key file.
    pub fn key_file(path: impl Into<PathBuf>, create: bool) -> Self {
        KeySource::KeyFile {
            path: path.into(),
            create,
        }
    }
}

impl std::fmt::Debug for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::Passphrase(_) => f.write_str("KeySource::Passphrase"),
            KeySource::KeyFile { path, create } => f
                .debug_struct("KeySource::KeyFile")
                .field("path", path)
                .field("create", create)
                .finish(),
            KeySource::Key(_) => f.write_str("KeySource::Key(***)"),
        }
    }
}

/// Keeps a derived key between runs so a login session unlocks once.
pub trait UnlockCache: Send + Sync {
    fn load(&self, store_id: &str) -> Option<Zeroizing<Vec<u8>>>;
    fn store(&self, store_id: &str, key: &[u8]);
    fn clear(&self, store_id: &str);
}

/// Process-memory cache: lasts as long as the process (an interactive REPL).
#[derive(Default)]
pub struct ProcessUnlockCache {
    keys: Mutex<HashMap<String, Zeroizing<Vec<u8>>>>,
}

impl ProcessUnlockCache {
    /// The cache shared by every store opened in this process.
    pub fn global() -> Arc<dyn UnlockCache> {
        static GLOBAL: std::sync::OnceLock<Arc<ProcessUnlockCache>> = std::sync::OnceLock::new();
        GLOBAL.get_or_init(Default::default).clone()
    }
}

impl UnlockCache for ProcessUnlockCache {
    fn load(&self, store_id: &str) -> Option<Zeroizing<Vec<u8>>> {
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(store_id)
            .cloned()
    }

    fn store(&self, store_id: &str, key: &[u8]) {
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(store_id.to_string(), Zeroizing::new(key.to_vec()));
    }

    fn clear(&self, store_id: &str) {
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(store_id);
    }
}

/// Linux kernel keyutils *session* keyring: the key survives across
/// `dbcrust` invocations of one login session and vanishes at logout.
#[cfg(target_os = "linux")]
pub struct KeyutilsUnlockCache;

#[cfg(target_os = "linux")]
impl KeyutilsUnlockCache {
    const SERVICE: &'static str = "dbcrust.unlock";

    fn entry(store_id: &str) -> Option<keyring::Entry> {
        keyring::Entry::new_with_target("session", Self::SERVICE, store_id).ok()
    }

    /// True when the kernel keyring accepts a write+read+delete round trip.
    pub fn is_available() -> bool {
        let Some(entry) = Self::entry("probe") else {
            return false;
        };
        let works = entry.set_secret(b"probe").is_ok()
            && entry.get_secret().map(|v| v == b"probe").unwrap_or(false);
        let _ = entry.delete_credential();
        works
    }
}

#[cfg(target_os = "linux")]
impl UnlockCache for KeyutilsUnlockCache {
    fn load(&self, store_id: &str) -> Option<Zeroizing<Vec<u8>>> {
        Self::entry(store_id)?.get_secret().ok().map(Zeroizing::new)
    }

    fn store(&self, store_id: &str, key: &[u8]) {
        if let Some(entry) = Self::entry(store_id) {
            let _ = entry.set_secret(key);
        }
    }

    fn clear(&self, store_id: &str) {
        if let Some(entry) = Self::entry(store_id) {
            let _ = entry.delete_credential();
        }
    }
}

/// The cache to use when `file_unlock_cache` is on: keyutils on Linux (no
/// caching at all when keyutils is unavailable there), process memory
/// elsewhere.
pub fn platform_unlock_cache() -> Option<Arc<dyn UnlockCache>> {
    #[cfg(target_os = "linux")]
    {
        if KeyutilsUnlockCache::is_available() {
            return Some(Arc::new(KeyutilsUnlockCache));
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some(ProcessUnlockCache::global())
    }
}

struct Header {
    version: u16,
    kdf: u8,
    params: KdfParams,
    salt: [u8; SALT_LEN],
    check_nonce: [u8; NONCE_LEN],
    check_ct: Vec<u8>,
}

impl Header {
    /// Everything before the check block: also its AAD.
    fn prefix(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 2 + 1 + 12 + SALT_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.version.to_be_bytes());
        out.push(self.kdf);
        out.extend_from_slice(&self.params.m_cost_kib.to_be_bytes());
        out.extend_from_slice(&self.params.t_cost.to_be_bytes());
        out.extend_from_slice(&self.params.p_cost.to_be_bytes());
        out.extend_from_slice(&self.salt);
        out
    }

    fn store_id(&self) -> String {
        let digest = Sha256::digest(self.salt);
        hex::encode(&digest[..16])
    }
}

struct Sealed {
    nonce: [u8; NONCE_LEN],
    ciphertext: Vec<u8>,
}

struct StoreFile {
    header: Header,
    records: BTreeMap<String, Sealed>,
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SecretError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| SecretError::Corrupt("the secrets file is truncated".to_string()))?;
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, SecretError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, SecretError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32, SecretError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
}

impl StoreFile {
    fn parse(bytes: &[u8]) -> Result<Self, SecretError> {
        let mut r = Reader { bytes, pos: 0 };
        if r.take(8)? != MAGIC {
            return Err(SecretError::Corrupt(
                "not a DBCrust secrets file (bad magic)".to_string(),
            ));
        }
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(SecretError::Corrupt(format!(
                "secrets file format version {version} is not supported by this DBCrust"
            )));
        }
        let kdf = r.u8()?;
        if kdf != KDF_NONE && kdf != KDF_ARGON2ID {
            return Err(SecretError::Corrupt("unknown key derivation".to_string()));
        }
        let params = KdfParams {
            m_cost_kib: r.u32()?,
            t_cost: r.u32()?,
            p_cost: r.u32()?,
        };
        if kdf == KDF_ARGON2ID {
            params.validate()?;
        }
        let salt: [u8; SALT_LEN] = r.take(SALT_LEN)?.try_into().expect("salt length");
        let check_nonce: [u8; NONCE_LEN] = r.take(NONCE_LEN)?.try_into().expect("nonce length");
        let check_ct = r.take(CHECK_PLAINTEXT.len() + TAG_LEN)?.to_vec();
        let count = r.u32()?;
        if count > MAX_RECORDS {
            return Err(SecretError::Corrupt("too many records".to_string()));
        }
        let mut records = BTreeMap::new();
        for _ in 0..count {
            let id_len = r.u16()? as usize;
            if id_len == 0 || id_len > MAX_ID_LEN {
                return Err(SecretError::Corrupt(
                    "record id length is invalid".to_string(),
                ));
            }
            let id = std::str::from_utf8(r.take(id_len)?)
                .map_err(|_| SecretError::Corrupt("record id is not valid text".to_string()))?
                .to_string();
            let nonce: [u8; NONCE_LEN] = r.take(NONCE_LEN)?.try_into().expect("nonce length");
            let ct_len = r.u32()? as usize;
            if !(TAG_LEN..=MAX_RECORD_CT).contains(&ct_len) {
                return Err(SecretError::Corrupt("record length is invalid".to_string()));
            }
            let ciphertext = r.take(ct_len)?.to_vec();
            if records.insert(id, Sealed { nonce, ciphertext }).is_some() {
                return Err(SecretError::Corrupt("duplicate record id".to_string()));
            }
        }
        if r.pos != bytes.len() {
            return Err(SecretError::Corrupt(
                "unexpected data after the last record".to_string(),
            ));
        }
        Ok(StoreFile {
            header: Header {
                version,
                kdf,
                params,
                salt,
                check_nonce,
                check_ct,
            },
            records,
        })
    }

    fn serialize(&self) -> Vec<u8> {
        let mut out = self.header.prefix();
        out.extend_from_slice(&self.header.check_nonce);
        out.extend_from_slice(&self.header.check_ct);
        out.extend_from_slice(&(self.records.len() as u32).to_be_bytes());
        for (id, sealed) in &self.records {
            out.extend_from_slice(&(id.len() as u16).to_be_bytes());
            out.extend_from_slice(id.as_bytes());
            out.extend_from_slice(&sealed.nonce);
            out.extend_from_slice(&(sealed.ciphertext.len() as u32).to_be_bytes());
            out.extend_from_slice(&sealed.ciphertext);
        }
        out
    }
}

fn cipher(key: &Key) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(&key[..]).expect("a 32-byte key is valid for AES-256")
}

fn record_aad(id: &str) -> Vec<u8> {
    let mut aad = Vec::with_capacity(2 + id.len());
    aad.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    aad.extend_from_slice(id.as_bytes());
    aad
}

fn seal(key: &Key, aad: &[u8], plaintext: &[u8]) -> Result<Sealed, SecretError> {
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher(key)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| SecretError::Io("encryption failed".to_string()))?;
    Ok(Sealed { nonce, ciphertext })
}

fn open(key: &Key, aad: &[u8], nonce: &[u8; NONCE_LEN], ct: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    cipher(key)
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
        .ok()
        .map(Zeroizing::new)
}

fn key_matches(key: &Key, header: &Header) -> bool {
    open(key, &header.prefix(), &header.check_nonce, &header.check_ct)
        .is_some_and(|plain| plain.as_slice() == CHECK_PLAINTEXT)
}

fn derive_key(passphrase: &str, salt: &[u8], params: &KdfParams) -> Result<Key, SecretError> {
    params.validate()?;
    let argon_params = Params::new(params.m_cost_kib, params.t_cost, params.p_cost, Some(32))
        .map_err(|_| SecretError::Corrupt("invalid key derivation parameters".to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut *key)
        .map_err(|_| SecretError::Io("key derivation failed".to_string()))?;
    Ok(key)
}

/// A single encrypted file holding many records.
///
/// Construct it over any path with any [`KeySource`]: the DBCrust config
/// directory with a passphrase (interactive users) or a broker state
/// directory with a key file (service mode).
pub struct PassphraseFileBackend {
    path: PathBuf,
    key_source: KeySource,
    kdf: KdfParams,
    unlock_cache: Option<Arc<dyn UnlockCache>>,
    memory_key: Mutex<Option<(String, Key)>>,
}

impl std::fmt::Debug for PassphraseFileBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PassphraseFileBackend")
            .field("path", &self.path)
            .field("key_source", &self.key_source)
            .finish()
    }
}

impl PassphraseFileBackend {
    /// Store at `path`, keyed by `key_source`, default KDF cost, no
    /// cross-process unlock cache.
    pub fn new(path: impl Into<PathBuf>, key_source: KeySource) -> Self {
        Self {
            path: path.into(),
            key_source,
            kdf: KdfParams::DEFAULT,
            unlock_cache: None,
            memory_key: Mutex::new(None),
        }
    }

    /// `<dir>/secrets.enc`.
    pub fn in_dir(dir: &Path, key_source: KeySource) -> Self {
        Self::new(dir.join(SECRETS_FILE_NAME), key_source)
    }

    /// KDF cost used when this backend *creates* a file (existing files keep
    /// the parameters in their header).
    pub fn with_kdf_params(mut self, params: KdfParams) -> Self {
        self.kdf = params;
        self
    }

    /// Cache derived keys between runs (see [`platform_unlock_cache`]).
    pub fn with_unlock_cache(mut self, cache: Option<Arc<dyn UnlockCache>>) -> Self {
        self.unlock_cache = cache;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<Option<StoreFile>, SecretError> {
        match fsutil::read_bounded(&self.path, MAX_FILE_BYTES)? {
            Some(bytes) => StoreFile::parse(&bytes).map(Some),
            None => Ok(None),
        }
    }

    fn remember(&self, store_id: &str, key: &Key) {
        *self.memory_key.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((store_id.to_string(), key.clone()));
        if let Some(cache) = &self.unlock_cache {
            cache.store(store_id, &key[..]);
        }
    }

    /// A key that is already available without prompting, if it verifies.
    fn cached_key(&self, header: &Header) -> Option<Key> {
        let store_id = header.store_id();
        let from_memory = self
            .memory_key
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .filter(|(id, _)| *id == store_id)
            .map(|(_, key)| key.clone());
        if let Some(key) = from_memory {
            if key_matches(&key, header) {
                return Some(key);
            }
            *self.memory_key.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        if let (Some(cache), KeySource::Passphrase(_)) = (&self.unlock_cache, &self.key_source) {
            if let Some(bytes) = cache.load(&store_id) {
                if bytes.len() == 32 {
                    let mut key = Zeroizing::new([0u8; 32]);
                    key.copy_from_slice(&bytes);
                    if key_matches(&key, header) {
                        return Some(key);
                    }
                }
                // Stale (file recreated or passphrase changed): drop it.
                cache.clear(&store_id);
            }
        }
        None
    }

    /// The key for an existing file. Prompts only for a passphrase source
    /// with no cached key.
    fn unlock_existing(&self, header: &Header) -> Result<Key, SecretError> {
        if let Some(key) = self.cached_key(header) {
            return Ok(key);
        }
        match &self.key_source {
            KeySource::Key(key) => {
                if header.kdf != KDF_NONE {
                    return Err(SecretError::Denied(
                        "this store is protected by a passphrase, not a key".to_string(),
                    ));
                }
                if key_matches(key, header) {
                    Ok(key.clone())
                } else {
                    Err(SecretError::Denied(
                        "the key does not open this store".to_string(),
                    ))
                }
            }
            KeySource::KeyFile { path, .. } => {
                if header.kdf != KDF_NONE {
                    return Err(SecretError::Denied(
                        "this store is protected by a passphrase, not a key file".to_string(),
                    ));
                }
                let key = fsutil::read_key_file(path)?;
                if key_matches(&key, header) {
                    Ok(key)
                } else {
                    Err(SecretError::Denied(
                        "the key file does not open this store".to_string(),
                    ))
                }
            }
            KeySource::Passphrase(provider) => {
                if header.kdf != KDF_ARGON2ID {
                    return Err(SecretError::Denied(
                        "this store was created with a key file, not a passphrase".to_string(),
                    ));
                }
                let attempts = if provider.can_retry() { 3 } else { 1 };
                for attempt in 1..=attempts {
                    let passphrase = provider.passphrase(false)?;
                    let key = derive_key(&passphrase, &header.salt, &header.params)?;
                    if key_matches(&key, header) {
                        self.remember(&header.store_id(), &key);
                        return Ok(key);
                    }
                    if attempt < attempts {
                        eprintln!("Wrong passphrase, try again.");
                    }
                }
                Err(SecretError::Denied("wrong passphrase".to_string()))
            }
        }
    }

    /// Header and key for a brand-new file.
    fn create_new(&self) -> Result<(StoreFile, Key), SecretError> {
        let mut salt = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let (kdf, params, key) = match &self.key_source {
            KeySource::Key(key) => (KDF_NONE, KdfParams::INSECURE_FOR_TESTS, key.clone()),
            KeySource::KeyFile { path, create } => {
                if !path.exists() {
                    if *create {
                        fsutil::create_key_file(path)?;
                    } else {
                        return Err(SecretError::Unavailable(format!(
                            "key file {} does not exist",
                            path.display()
                        )));
                    }
                }
                (
                    KDF_NONE,
                    KdfParams::INSECURE_FOR_TESTS,
                    fsutil::read_key_file(path)?,
                )
            }
            KeySource::Passphrase(provider) => {
                let passphrase = provider.passphrase(true)?;
                (
                    KDF_ARGON2ID,
                    self.kdf,
                    derive_key(&passphrase, &salt, &self.kdf)?,
                )
            }
        };
        let mut header = Header {
            version: FORMAT_VERSION,
            kdf,
            params,
            salt,
            check_nonce: [0; NONCE_LEN],
            check_ct: Vec::new(),
        };
        let check = seal(&key, &header.prefix(), CHECK_PLAINTEXT)?;
        header.check_nonce = check.nonce;
        header.check_ct = check.ciphertext;
        self.remember(&header.store_id(), &key);
        Ok((
            StoreFile {
                header,
                records: BTreeMap::new(),
            },
            key,
        ))
    }

    fn write(&self, file: &StoreFile) -> Result<(), SecretError> {
        fsutil::write_private_atomic(&self.path, &file.serialize())
    }
}

impl SecretBackend for PassphraseFileBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::EncryptedFile
    }

    fn get(&self, id: &str) -> Result<Option<Zeroizing<String>>, SecretError> {
        let Some(file) = self.load()? else {
            return Ok(None);
        };
        // Ids are plaintext: a miss needs no key and never prompts.
        let Some(sealed) = file.records.get(id) else {
            return Ok(None);
        };
        let key = self.unlock_existing(&file.header)?;
        let plain =
            open(&key, &record_aad(id), &sealed.nonce, &sealed.ciphertext).ok_or_else(|| {
                SecretError::Corrupt(format!("record {id} failed its integrity check"))
            })?;
        std::str::from_utf8(&plain)
            .map(|text| Some(Zeroizing::new(text.to_string())))
            .map_err(|_| SecretError::Corrupt(format!("record {id} is not valid text")))
    }

    fn put(&self, id: &str, value: &str) -> Result<(), SecretError> {
        if id.is_empty() || id.len() > MAX_ID_LEN {
            return Err(SecretError::Io("record id length is invalid".to_string()));
        }
        let _lock = FileLock::acquire(&self.path)?;
        let (mut file, key) = match self.load()? {
            Some(file) => {
                let key = self.unlock_existing(&file.header)?;
                (file, key)
            }
            None => self.create_new()?,
        };
        let sealed = seal(&key, &record_aad(id), value.as_bytes())?;
        file.records.insert(id.to_string(), sealed);
        self.write(&file)
    }

    fn delete(&self, id: &str) -> Result<bool, SecretError> {
        let _lock = FileLock::acquire(&self.path)?;
        let Some(mut file) = self.load()? else {
            return Ok(false);
        };
        if !file.records.contains_key(id) {
            return Ok(false);
        }
        // Mutating the store needs the key, like writing does.
        self.unlock_existing(&file.header)?;
        file.records.remove(id);
        self.write(&file)?;
        Ok(true)
    }

    fn status(&self) -> BackendStatus {
        let detail_path = self.path.display().to_string();
        let file = match self.load() {
            Ok(Some(file)) => file,
            Ok(None) => {
                return BackendStatus {
                    kind: BackendKind::EncryptedFile,
                    state: BackendState::Empty,
                    detail: format!("{detail_path} (created on first save)"),
                };
            }
            Err(e) => {
                return BackendStatus {
                    kind: BackendKind::EncryptedFile,
                    state: BackendState::Unavailable,
                    detail: format!("{detail_path}: {e}"),
                };
            }
        };
        let (state, note) = match &self.key_source {
            KeySource::Passphrase(_) => {
                if self.cached_key(&file.header).is_some() {
                    (BackendState::Ready, "unlocked")
                } else {
                    (BackendState::Locked, "locked")
                }
            }
            KeySource::Key(_) | KeySource::KeyFile { .. } => {
                match self.unlock_existing(&file.header) {
                    Ok(_) => (BackendState::Ready, "key available"),
                    Err(_) => (BackendState::Unavailable, "key does not open the store"),
                }
            }
        };
        BackendStatus {
            kind: BackendKind::EncryptedFile,
            state,
            detail: format!("{detail_path}, {} record(s), {note}", file.records.len()),
        }
    }

    fn list_ids(&self) -> Result<Option<Vec<String>>, SecretError> {
        Ok(Some(
            self.load()?
                .map(|file| file.records.keys().cloned().collect())
                .unwrap_or_default(),
        ))
    }

    fn unlock(&self) -> Result<(), SecretError> {
        match self.load()? {
            Some(file) => self.unlock_existing(&file.header).map(|_| ()),
            None => Ok(()),
        }
    }

    fn lock(&self) -> Result<(), SecretError> {
        let store_id = self
            .memory_key
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map(|(id, _)| id)
            .or_else(|| {
                self.load()
                    .ok()
                    .flatten()
                    .map(|file| file.header.store_id())
            });
        if let (Some(cache), Some(store_id)) = (&self.unlock_cache, store_id) {
            cache.clear(&store_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passphrase_backend(dir: &Path, passphrase: &str) -> PassphraseFileBackend {
        PassphraseFileBackend::in_dir(
            dir,
            KeySource::Passphrase(Arc::new(FixedPassphrase::new(passphrase))),
        )
        .with_kdf_params(KdfParams::INSECURE_FOR_TESTS)
    }

    #[test]
    fn round_trip_overwrite_delete_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let backend = passphrase_backend(dir.path(), "correct horse");
        assert!(backend.get("conn:a").unwrap().is_none());

        backend.put("conn:a", "pw-one").unwrap();
        backend.put("ai:openai", "sk-canary").unwrap();
        assert_eq!(&*backend.get("conn:a").unwrap().unwrap(), "pw-one");

        backend.put("conn:a", "pw-two").unwrap();
        // A second backend instance (new process) reads what was written.
        let again = passphrase_backend(dir.path(), "correct horse");
        assert_eq!(&*again.get("conn:a").unwrap().unwrap(), "pw-two");
        assert_eq!(&*again.get("ai:openai").unwrap().unwrap(), "sk-canary");
        assert_eq!(
            again.list_ids().unwrap().unwrap(),
            vec!["ai:openai".to_string(), "conn:a".to_string()]
        );

        assert!(again.delete("conn:a").unwrap());
        assert!(!again.delete("conn:a").unwrap());
        assert!(again.get("conn:a").unwrap().is_none());

        let raw = std::fs::read(dir.path().join(SECRETS_FILE_NAME)).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(!text.contains("pw-two") && !text.contains("sk-canary"));
        assert!(raw.starts_with(MAGIC));
    }

    #[cfg(unix)]
    #[test]
    fn file_is_private_and_header_records_kdf_params() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let backend = passphrase_backend(dir.path(), "correct horse");
        backend.put("conn:a", "x").unwrap();
        let path = dir.path().join(SECRETS_FILE_NAME);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let parsed = StoreFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed.header.kdf, KDF_ARGON2ID);
        assert_eq!(parsed.header.params, KdfParams::INSECURE_FOR_TESTS);
        assert_eq!(parsed.header.version, FORMAT_VERSION);
    }

    #[test]
    fn wrong_passphrase_is_denied_without_panic_and_leaks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        passphrase_backend(dir.path(), "right-passphrase")
            .put("conn:a", "CANARY-VALUE")
            .unwrap();
        let wrong = passphrase_backend(dir.path(), "wrong-passphrase");
        let err = wrong.get("conn:a").unwrap_err();
        assert!(matches!(err, SecretError::Denied(_)), "{err:?}");
        assert!(!err.to_string().contains("CANARY"));
        assert!(wrong.put("conn:b", "x").is_err());
        // A miss still needs no key.
        assert!(wrong.get("conn:missing").unwrap().is_none());
        assert_eq!(wrong.status().state, BackendState::Locked);
    }

    #[test]
    fn corrupt_files_are_reported_not_panicked_on() {
        let dir = tempfile::tempdir().unwrap();
        let backend = passphrase_backend(dir.path(), "correct horse");
        backend.put("conn:a", "pw").unwrap();
        let path = dir.path().join(SECRETS_FILE_NAME);
        let good = std::fs::read(&path).unwrap();

        // Truncated at every length: never a panic, always Corrupt.
        for len in 0..good.len() {
            std::fs::write(&path, &good[..len]).unwrap();
            let err = backend.get("conn:a").unwrap_err();
            assert!(matches!(err, SecretError::Corrupt(_)), "len {len}: {err:?}");
        }
        // Bad magic, bad version, trailing junk.
        let mut bad = good.clone();
        bad[0] ^= 0xff;
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(
            backend.get("conn:a"),
            Err(SecretError::Corrupt(_))
        ));
        let mut bad = good.clone();
        bad[9] = 7;
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(
            backend.get("conn:a"),
            Err(SecretError::Corrupt(_))
        ));
        let mut bad = good.clone();
        bad.push(0);
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(
            backend.get("conn:a"),
            Err(SecretError::Corrupt(_))
        ));
        // Flipped ciphertext byte: record integrity check fails.
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0x01;
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(
            backend.get("conn:a"),
            Err(SecretError::Corrupt(_))
        ));
        // Hostile KDF parameters are refused before any allocation.
        let mut bad = good.clone();
        bad[12..16].copy_from_slice(&u32::MAX.to_be_bytes());
        std::fs::write(&path, &bad).unwrap();
        assert!(matches!(
            backend.get("conn:a"),
            Err(SecretError::Corrupt(_))
        ));
    }

    #[test]
    fn moving_a_ciphertext_to_another_id_fails_the_aad_check() {
        let dir = tempfile::tempdir().unwrap();
        let backend = passphrase_backend(dir.path(), "correct horse");
        backend.put("conn:cheap", "low-value").unwrap();
        backend.put("conn:prod", "high-value").unwrap();
        let path = dir.path().join(SECRETS_FILE_NAME);
        let mut file = StoreFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let cheap = file.records.remove("conn:cheap").unwrap();
        file.records.insert("conn:prod".to_string(), cheap);
        std::fs::write(&path, file.serialize()).unwrap();

        let err = backend.get("conn:prod").unwrap_err();
        assert!(matches!(err, SecretError::Corrupt(_)), "{err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn key_file_permissions_are_enforced() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let key_path = dir.path().join("store.key");
        let backend = PassphraseFileBackend::new(
            dir.path().join("secrets.enc"),
            KeySource::key_file(&key_path, true),
        );
        backend.put("broker:prod", "pw").unwrap();
        assert!(key_path.exists());
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(&*backend.get("broker:prod").unwrap().unwrap(), "pw");

        // Group/other readable key file: refused.
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let fresh = PassphraseFileBackend::new(
            dir.path().join("secrets.enc"),
            KeySource::key_file(&key_path, false),
        );
        assert!(matches!(
            fresh.get("broker:prod"),
            Err(SecretError::Denied(_))
        ));
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(fresh.get("broker:prod").unwrap().is_some());

        // World-accessible directory: refused.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            fresh.get("broker:prod"),
            Err(SecretError::Denied(_))
        ));
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();

        // A symlinked key file is never followed.
        let link = dir.path().join("link.key");
        std::os::unix::fs::symlink(&key_path, &link).unwrap();
        let linked = PassphraseFileBackend::new(
            dir.path().join("secrets.enc"),
            KeySource::key_file(&link, false),
        );
        assert!(matches!(
            linked.get("broker:prod"),
            Err(SecretError::Denied(_))
        ));
    }

    #[test]
    fn a_passphrase_source_cannot_open_a_key_file_store_and_vice_versa() {
        let dir = tempfile::tempdir().unwrap();
        let key = Zeroizing::new([7u8; 32]);
        let keyed = PassphraseFileBackend::in_dir(dir.path(), KeySource::Key(key.clone()));
        keyed.put("conn:a", "pw").unwrap();
        let pass = passphrase_backend(dir.path(), "correct horse");
        assert!(matches!(pass.get("conn:a"), Err(SecretError::Denied(_))));
        let wrong_key =
            PassphraseFileBackend::in_dir(dir.path(), KeySource::Key(Zeroizing::new([8u8; 32])));
        assert!(matches!(
            wrong_key.get("conn:a"),
            Err(SecretError::Denied(_))
        ));
    }

    #[test]
    fn symlinked_store_file_is_refused() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("elsewhere");
            std::fs::write(&target, b"x").unwrap();
            std::os::unix::fs::symlink(&target, dir.path().join(SECRETS_FILE_NAME)).unwrap();
            let backend = passphrase_backend(dir.path(), "correct horse");
            assert!(matches!(backend.get("conn:a"), Err(SecretError::Denied(_))));
            assert!(backend.put("conn:a", "x").is_err());
            assert_eq!(std::fs::read(&target).unwrap(), b"x");
        }
    }

    #[test]
    fn unlock_cache_unlocks_once_and_lock_clears_it() {
        struct CountingProvider(std::sync::atomic::AtomicUsize);
        impl PassphraseProvider for CountingProvider {
            fn passphrase(&self, _confirm: bool) -> Result<Zeroizing<String>, SecretError> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Zeroizing::new("correct horse".to_string()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let cache: Arc<dyn UnlockCache> = Arc::new(ProcessUnlockCache::default());
        let make = |provider: Arc<CountingProvider>| {
            PassphraseFileBackend::in_dir(dir.path(), KeySource::Passphrase(provider))
                .with_kdf_params(KdfParams::INSECURE_FOR_TESTS)
                .with_unlock_cache(Some(cache.clone()))
        };

        let first = Arc::new(CountingProvider(Default::default()));
        make(first.clone()).put("conn:a", "pw").unwrap(); // creation asks once
        assert_eq!(first.0.load(std::sync::atomic::Ordering::SeqCst), 1);

        // A "new process" with the same cache needs no prompt at all.
        let second = Arc::new(CountingProvider(Default::default()));
        let backend = make(second.clone());
        assert_eq!(backend.status().state, BackendState::Ready);
        assert_eq!(&*backend.get("conn:a").unwrap().unwrap(), "pw");
        assert_eq!(second.0.load(std::sync::atomic::Ordering::SeqCst), 0);

        backend.lock().unwrap();
        let third = Arc::new(CountingProvider(Default::default()));
        let relocked = make(third.clone());
        assert_eq!(relocked.status().state, BackendState::Locked);
        assert_eq!(&*relocked.get("conn:a").unwrap().unwrap(), "pw");
        assert_eq!(third.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn non_interactive_prompt_is_locked_not_a_hang() {
        // Test processes have no TTY on stdin; the prompt provider must
        // return Locked instead of blocking.
        super::super::set_interactive(false);
        let dir = tempfile::tempdir().unwrap();
        passphrase_backend(dir.path(), "correct horse")
            .put("conn:a", "pw")
            .unwrap();
        let backend = PassphraseFileBackend::in_dir(dir.path(), KeySource::prompt());
        let err = backend.get("conn:a").unwrap_err();
        assert!(err.is_locked(), "{err:?}");
        assert!(err.to_string().contains("dbcrust secrets unlock"));
        let err =
            PassphraseFileBackend::in_dir(tempfile::tempdir().unwrap().path(), KeySource::prompt())
                .put("conn:a", "pw")
                .unwrap_err();
        assert!(err.is_locked(), "{err:?}");
    }

    #[test]
    fn debug_never_shows_key_material() {
        let source = KeySource::Key(Zeroizing::new([0xAB; 32]));
        assert!(!format!("{source:?}").contains("ab"));
        let dir = tempfile::tempdir().unwrap();
        let backend = passphrase_backend(dir.path(), "correct horse");
        assert!(!format!("{backend:?}").contains("correct"));
    }
}
