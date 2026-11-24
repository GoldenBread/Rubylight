//! Nucleus Co-op's change to a game's Windows compatibility settings, undone. On Linux
//! there are no such settings and no session ever changes one: a user's settings cannot be
//! opened, and putting back what sessions left finds nothing.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// The compatibility layers in a user's registry hive: one value per program path.
pub const KEY: &str = r"Software\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers";

/// A registry value: its type and bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Value {
    pub kind: u32,
    pub data: Vec<u8>,
}

/// One user's compatibility layers: their hive on a Windows PC, a map in tests.
pub trait Layers: Send {
    /// The user's SID.
    fn user(&self) -> &str;
    /// Every value by name; none without the key.
    fn values(&mut self) -> Result<BTreeMap<String, Value>>;
    fn set(&mut self, name: &str, value: &Value) -> Result<()>;
    /// Remove a value; one that is gone already is fine.
    fn delete(&mut self, name: &str) -> Result<()>;
}

/// What every use of a user's compatibility settings answers with on Linux.
fn no_layers() -> anyhow::Error {
    crate::unsupported("Windows compatibility settings")
}

/// Before the host runs another Nucleus Co-op than the one at `folder`: put back what
/// sessions left in it. No session runs on Linux, so nothing was left and this succeeds
/// without reading the folder.
pub fn put_back_to_leave(_folder: &Path) -> Result<()> {
    Ok(())
}

/// A user's layers in their registry hive. None can be opened on Linux, so no value of
/// this type exists there.
pub struct UserHive {
    sid: String,
}

impl UserHive {
    /// The user the host runs Nucleus Co-op as. Always an error on Linux.
    pub fn signed_in() -> Result<Self> {
        Err(no_layers())
    }
}

impl Layers for UserHive {
    fn user(&self) -> &str {
        &self.sid
    }
    fn values(&mut self) -> Result<BTreeMap<String, Value>> {
        Err(no_layers())
    }
    fn set(&mut self, _name: &str, _value: &Value) -> Result<()> {
        Err(no_layers())
    }
    fn delete(&mut self, _name: &str) -> Result<()> {
        Err(no_layers())
    }
}
