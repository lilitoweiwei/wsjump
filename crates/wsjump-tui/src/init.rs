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
# UPGRADE:  after updating wsjump-tui, re-run the INSTALL line and replace the
#           block below, so the `ju` function stays in sync with the tools.
#
# UNINSTALL: delete that same line from your profile.
# ===================================================================

# `ju`: the workspace tool. With no arguments it opens the interactive picker
# and jumps to the chosen workspace; a command word runs the matching `wsjump`
# command instead. Single letters and full names both work.
function ju {
    param([Parameter(ValueFromRemainingArguments = $true)][string[]]$argv)

    if (-not $argv -or $argv.Count -eq 0) {
        # Launch the interactive picker; it draws its UI on stderr and prints the
        # chosen workspace path on stdout. Jump only when a path was returned.
        $dir = wsjump-tui
        if ($LASTEXITCODE -eq 0 -and $dir) {
            Set-Location $dir
        }
        return
    }

    $cmd = $argv[0]
    $rest = @()
    if ($argv.Count -gt 1) {
        $rest = @($argv[1..($argv.Count - 1)])
    }

    switch ($cmd) {
        { $_ -in 'n', 'new' } {
            # Create the workspace (its path is printed), then jump into it.
            $dir = wsjump new @rest
            if ($LASTEXITCODE -eq 0 -and $dir) {
                Set-Location $dir
            }
        }
        { $_ -in 't', 'touch' } {
            # Record a visit; with no path, the current directory.
            wsjump touch @rest
        }
        { $_ -in 's', 'sync' } {
            # Force a full re-read of every workspace's metadata.
            wsjump sync @rest
        }
        { $_ -in '-h', '--help' } {
            $usage = @(
                'usage:'
                '  ju                 jump interactively'
                '  ju n|new [slug...] create a workspace and jump into it'
                '  ju t|touch [path]  record a visit (default: the current directory)'
                '  ju s|sync [path]   force a full re-read of workspace metadata'
            )
            $usage | Write-Host
        }
        default {
            # An error, so render it as one: red, and not on the success stream,
            # which this function's callers read for a path.
            $Host.UI.WriteErrorLine("ju: unknown command '$cmd'")
            $Host.UI.WriteErrorLine("run 'ju -h' for usage")
        }
    }
}"#;

/// Print the integration snippet to stdout.
pub fn run() -> AppResult<()> {
    println!("{}", SNIPPET);
    Ok(())
}
