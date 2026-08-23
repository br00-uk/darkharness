//! Options that control the discovery walk.

/// The default file size limit, in bytes. Files larger than this are
/// excluded. See F1, "Do" item 2.
pub const DEFAULT_MAX_FILE_SIZE: u64 = 1_048_576;

/// The extra ignore file that the harness reads alongside `.gitignore`.
///
/// A `.darkignore` file uses the same pattern syntax as `.gitignore`,
/// negation included, and it applies at the same directory scope.
pub const DARKIGNORE_FILENAME: &str = ".darkignore";

/// The vendored directory names that discovery excludes by default.
pub const DEFAULT_VENDOR_DIRS: &[&str] = &["vendor", "node_modules", "third_party"];

/// The number of leading bytes that the binary-file test reads.
///
/// Discovery treats a file as binary when a NUL byte appears in this many
/// bytes from the start of the file. See F1, "Do" item 2.
pub const NUL_SCAN_WINDOW: usize = 8192;

/// Options for [`super::discover`].
///
/// Build one with [`DiscoverOptions::default`] and adjust the fields that
/// need to differ from the repository default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverOptions {
    /// The largest file, in bytes, that discovery includes. Discovery
    /// excludes a file whose size exceeds this value.
    pub max_file_size: u64,
    /// The directory names that discovery excludes, wherever they appear in
    /// the tree. The default list is `vendor`, `node_modules`, and
    /// `third_party`.
    pub vendor_dirs: Vec<String>,
}

impl Default for DiscoverOptions {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            vendor_dirs: DEFAULT_VENDOR_DIRS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        }
    }
}
