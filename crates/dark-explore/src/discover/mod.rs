//! The `discover` stage.
//!
//! Discovery walks the repository from its root, drops what `.gitignore`,
//! `.darkignore`, size, and binary-content filters exclude, and returns a
//! [`Snapshot`] whose file list is sorted with a byte comparator (Rule 30)
//! and hashed with no timestamp in the mix (Rule 31). Running discovery
//! twice over the same commit and the same [`DiscoverOptions`] produces the
//! same `Snapshot`, byte for byte (Rule 29). See task unit `F1`.

mod file;
mod options;
mod order;
mod walk;

pub use file::DiscoveredFile;
pub use options::{
    DARKIGNORE_FILENAME, DEFAULT_MAX_FILE_SIZE, DEFAULT_VENDOR_DIRS, DiscoverOptions,
    NUL_SCAN_WINDOW,
};
pub use order::compare_paths;
pub use walk::{Snapshot, discover};
