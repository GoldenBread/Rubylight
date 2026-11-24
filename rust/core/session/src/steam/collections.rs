//! Steam cloud-storage collections and last-known fallback state.
use super::{StampCache, Vdf, only_on_this_pc, read_capped};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{LazyLock, PoisonError},
};

/// What Steam's collections said: their names by app, and what got in the way of reading them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collections {
    /// Each app's collections by name, in file order.
    pub apps: BTreeMap<u32, Vec<String>>,
    /// Why some or all of them could not be read, for one log line.
    pub problem: Option<String>,
    /// The file they came from, `None` when no account has one.
    pub file: Option<PathBuf>,
    /// The file could not be read at all, so `apps` says nothing about it ([`LastCollections`]).
    pub failed: bool,
}

/// The scans in a row that may fail to read a file before its last collections are dropped, the last
/// of them dropping them: past a moment's rewrite, old names must not stay until the host restarts.
pub const MAX_FAILED_COLLECTION_READS: u32 = 3;

/// The last collections a file gave. A file being rewritten can fail to read for a moment, and that
/// must not take every collection group off Home until the next scan.
#[derive(Default)]
pub struct LastCollections(Option<LastRead>);
struct LastRead {
    file: PathBuf,
    apps: BTreeMap<u32, Vec<String>>,
    failures: u32,
}
impl LastCollections {
    /// `read`, or the last collections its file gave when it failed, for fewer scans in a row than
    /// [`MAX_FAILED_COLLECTION_READS`]. A file that reads replaces them; another account's file or
    /// none forgets them.
    pub fn keeping(&mut self, mut read: Collections) -> Collections {
        match &read.file {
            Some(file) if read.failed => match &mut self.0 {
                Some(last) if last.file == *file => {
                    last.failures += 1;
                    if last.failures < MAX_FAILED_COLLECTION_READS {
                        read.apps = last.apps.clone();
                    } else {
                        self.0 = None;
                    }
                }
                _ => self.0 = None,
            },
            Some(file) => {
                self.0 = Some(LastRead {
                    file: file.clone(),
                    apps: read.apps.clone(),
                    failures: 0,
                })
            }
            None => self.0 = None,
        }
        read
    }
}
impl Collections {
    /// The one line for [`Self::problem`]: what it means for the library right now.
    pub fn problem_line(&self) -> &'static str {
        if self.failed && !self.apps.is_empty() {
            "Steam's collections could not be read, the library keeps the last ones for a few scans"
        } else if self.failed {
            "Steam's collections could not be read, the library has none"
        } else {
            "some of Steam's collections could not be read and are missing from the library"
        }
    }
}

/// The cloud storage file that holds an account's collections.
pub(super) const COLLECTIONS_FILE: &str = "cloud-storage-namespace-1.json";

/// The account the last read of the file gave, with the file's size and time it was read.
static MOST_RECENT: LazyLock<StampCache<Option<String>>> = LazyLock::new(Default::default);
/// The `userdata` folder of the account `loginusers.vdf` marks `MostRecent`: a SteamID64's low 32
/// bits. Not every Steam writes the mark. The file is read again only when its size or time moved
/// since the last read: the library checks ask every few seconds and it changes only when another
/// account signs in.
fn most_recent_account(root: &Path) -> Option<String> {
    let file = root.join("config").join("loginusers.vdf");
    let stamp = std::fs::metadata(&file)
        .ok()
        .and_then(|metadata| Some((metadata.len(), metadata.modified().ok()?)));
    let Some(stamp) = stamp else {
        // No file to cache by: read it, as before.
        return read_capped(&file).ok().and_then(|text| marked(&text));
    };
    let mut read = MOST_RECENT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((last, account)) = read.get(root)
        && *last == stamp
    {
        return account.clone();
    }
    let Some(text) = read_capped(&file).ok() else {
        // Unreadable now, so not cached: the next check reads it again.
        return None;
    };
    let account = marked(&text);
    read.insert(root.to_owned(), (stamp, account.clone()));
    account
}
/// The account `loginusers.vdf`'s text marks, `None` when it marks none.
fn marked(text: &str) -> Option<String> {
    let document = Vdf::parse(text);
    let (id, _) = document
        .get("users")?
        .entries()
        .iter()
        .find(|(_, user)| user.get("MostRecent").and_then(Vdf::text) == Some("1"))?;
    Some((id.parse::<u64>().ok()? & 0xFFFF_FFFF).to_string())
}

/// The collections file of the account `loginusers.vdf` marks as signed in last, when it has one;
/// else the one Steam wrote last, of any account.
pub(super) fn collections_file(roots: &[PathBuf]) -> Option<PathBuf> {
    let in_account = |user: PathBuf| {
        user.join("config")
            .join("cloudstorage")
            .join(COLLECTIONS_FILE)
    };
    let roots = only_on_this_pc(roots);
    roots
        .iter()
        .find_map(|root| {
            let account = most_recent_account(root)?;
            let file = in_account(root.join("userdata").join(account));
            file.is_file().then_some(file)
        })
        .or_else(|| {
            roots
                .iter()
                .filter_map(|root| std::fs::read_dir(root.join("userdata")).ok())
                .flat_map(|users| users.flatten())
                .map(|user| in_account(user.path()))
                .filter_map(|file| {
                    let modified = std::fs::metadata(&file).ok()?.modified().ok()?;
                    Some((modified, file))
                })
                .max_by_key(|(modified, _)| *modified)
                .map(|(_, file)| file)
        })
}

/// The collections of the Steam account [`collections_file`] picks. Steam's format is undocumented
/// and this never writes it: anything missing, malformed or changed leaves fewer collections and a
/// [`Collections::problem`], never an error. A PC with no Steam has no problem to tell.
pub fn collections(roots: &[PathBuf]) -> Collections {
    let Some(file) = collections_file(roots) else {
        let steam = only_on_this_pc(roots)
            .iter()
            .any(|root| root.join("userdata").is_dir());
        return Collections {
            problem: steam.then(|| format!("no account has {COLLECTIONS_FILE}")),
            ..Default::default()
        };
    };
    let read = match read_capped(&file) {
        Ok(text) => parse_collections(&text),
        Err(error) => Collections {
            problem: Some(format!("{COLLECTIONS_FILE} could not be read: {error}")),
            failed: true,
            ..Default::default()
        },
    };
    Collections {
        file: Some(file),
        ..read
    }
}

/// One cloud storage file: an array of `[key, {value, is_deleted, ..}]` pairs, where the value of a
/// `user-collections.<id>` key is JSON text of `{id, name, added: [appid], removed: [appid]}`.
/// Steam's own Hidden collection is no group. A dynamic collection's filter is left unread: its
/// group holds the apps it lists itself.
pub(super) fn parse_collections(text: &str) -> Collections {
    let mut collections = Collections::default();
    let entries = match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(entries)) => entries,
        Ok(_) => {
            collections.problem = Some(format!("{COLLECTIONS_FILE} is not a list"));
            collections.failed = true;
            return collections;
        }
        Err(error) => {
            collections.problem = Some(format!("{COLLECTIONS_FILE} is not JSON: {error}"));
            collections.failed = true;
            return collections;
        }
    };
    let mut unread = 0;
    for entry in &entries {
        let key = entry.get(0).and_then(Value::as_str).unwrap_or("");
        if !key.starts_with("user-collections.") || key == "user-collections.hidden" {
            continue;
        }
        let fields = entry.get(1);
        if fields
            .and_then(|fields| fields.get("is_deleted"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            continue;
        }
        let collection = fields
            .and_then(|fields| fields.get("value"))
            .and_then(Value::as_str)
            .and_then(|value| serde_json::from_str::<Value>(value).ok());
        let name = collection
            .as_ref()
            .and_then(|collection| collection.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let ids = |field: &str| -> Option<BTreeSet<u32>> {
            collection
                .as_ref()?
                .get(field)?
                .as_array()?
                .iter()
                .map(|id| id.as_u64().and_then(|id| u32::try_from(id).ok()))
                .collect()
        };
        let (Some(name), Some(added)) = (name, ids("added")) else {
            unread += 1;
            continue;
        };
        let removed = ids("removed").unwrap_or_default();
        for appid in added.difference(&removed) {
            collections
                .apps
                .entry(*appid)
                .or_default()
                .push(name.to_owned());
        }
    }
    if unread > 0 {
        collections.problem = Some(format!("{unread} collections could not be read"));
    }
    collections
}
