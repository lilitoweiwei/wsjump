//! Path rendering helpers.

use std::path::Path;

/// Render a path as a string using '/' as the separator.
///
/// On Windows, `Path` mixes separators when a config value using '/' is joined
/// with components (which use '\'). '\' is never a valid filename character on
/// Windows, so replacing it with '/' is safe and yields consistent output that
/// is less error-prone to consume.
pub fn display(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
