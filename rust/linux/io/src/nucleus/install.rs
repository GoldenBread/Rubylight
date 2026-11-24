//! Nucleus Co-op on this PC, on Linux: no folder is ever accepted as an install, since the
//! program does not run here. [`Found`] and its notice are as on Windows.
use anyhow::Result;
use butterpollo_session::nucleus::install::{self as pinned, Unusable};
use std::path::{Path, PathBuf};

/// The Nucleus folder streams use. Always none on Linux, also with a folder configured.
pub fn installed(data: &Path, configured: &str) -> Result<Option<PathBuf>> {
    Ok(found(data, configured)?.map(|found| found.folder))
}

/// The Nucleus folder [`installed`] names, and why `nucleus_path` doesn't name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub folder: PathBuf,
    /// Why the folder chosen is not used: it holds another release, and the copy an
    /// earlier Rubylight installed runs instead.
    pub instead: Option<Unusable>,
}
impl Found {
    /// What the console says while the earlier copy runs instead of the folder chosen.
    pub fn notice(&self) -> Option<String> {
        self.instead.as_ref().map(|why| {
            format!(
                "{why}. Until then Rubylight runs the Nucleus Co-op {} an earlier Rubylight installed, in {}",
                pinned::VERSION,
                self.folder.display()
            )
        })
    }
}

/// [`installed`] with why the folder chosen is not used. Always none on Linux: neither the
/// folder `configured` names nor one below `data` is checked or used.
pub fn found(_data: &Path, _configured: &str) -> Result<Option<Found>> {
    Ok(None)
}
