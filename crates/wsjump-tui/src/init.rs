//! `wsjump-tui init`: print the PowerShell shell-integration snippet.
//!
//! The output doubles as install guide (comments) and executable definition
//! (the `ju` function). Comments are harmless no-ops when the snippet is run
//! via `Invoke-Expression`, so the same output serves both reading and
//! execution.

use wsjump_core::error::AppResult;

/// The PowerShell integration snippet: install/uninstall guide + `ju` function.
const SNIPPET: &str = r#"# ===================================================================
# wsjump shell integration (PowerShell)
#
# INSTALL:  add the following line to your PowerShell profile
#           (run `notepad $PROFILE` to open it):
#
#               Invoke-Expression (& wsjump-tui init | Out-String)
#
# UNINSTALL: delete that same line from your profile.
# ===================================================================

function ju {
    # Launch the interactive picker; it draws its UI on stderr and prints the
    # chosen workspace path on stdout. Jump only when a path was returned.
    $dir = wsjump-tui
    if ($LASTEXITCODE -eq 0 -and $dir) {
        Set-Location $dir
    }
}"#;

/// Print the integration snippet to stdout.
pub fn run() -> AppResult<()> {
    println!("{}", SNIPPET);
    Ok(())
}
