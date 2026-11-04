//! `wsjump-tui init [<shell>]`: print the shell-integration snippet.
//!
//! Each snippet doubles as install guide (comments) and executable definition
//! (the `ju` function). Comments are harmless no-ops when the snippet is run
//! through the shell's eval (`Invoke-Expression` on PowerShell, `eval` on
//! bash), so the same output serves both reading and execution.

use crate::cli::Shell;
use wsjump_core::error::AppResult;

/// The PowerShell integration snippet: install/uninstall guide + `ju` function.
const POWERSHELL_SNIPPET: &str = r#"# ===================================================================
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
            Set-Location -LiteralPath $dir
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
                Set-Location -LiteralPath $dir
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
                '  ju n|new <name> [categories] create a workspace and jump into it'
                '  ju t|touch [path]  record a visit (default: the current directory)'
                '  ju s|sync [path]   force a full re-read of workspace metadata'
                '  example: ju new my-plugin _ai/_opencode (creates missing categories)'
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

/// The bash integration snippet: install/uninstall guide + `ju` function.
const BASH_SNIPPET: &str = r#"# ===================================================================
# wsjump shell integration (bash)
#
# INSTALL:  put `wsjump`, `wsjump-tui` and `wsjump.toml` into a directory on
#           your PATH (see the README "shell 集成" section), then add the
#           following line to your ~/.bashrc:
#
#               eval "$(wsjump-tui init bash)"
#
# UPGRADE:  after updating wsjump-tui, re-run the INSTALL line and re-source
#           ~/.bashrc, so the `ju` function stays in sync with the tools.
#
# UNINSTALL: delete that same line from ~/.bashrc.
# ===================================================================

# `ju`: the workspace tool. With no arguments it opens the interactive picker
# and jumps to the chosen workspace; a command word runs the matching `wsjump`
# command instead. Single letters and full names both work.
ju() {
    local dir tmp
    if [ "$#" -eq 0 ]; then
        # The picker must keep stdout on the terminal: the inline viewport's
        # cursor query (ESC [ 6 n) is answered only while stdout reaches the
        # terminal, and a command substitution would pipe it away. The chosen
        # path is written to a temp file instead.
        tmp="$(mktemp)" || return 1
        if wsjump-tui --output "$tmp"; then
            dir="$(cat "$tmp")"
            [ -n "$dir" ] && cd -- "$dir"
        fi
        rm -f "$tmp"
        return
    fi

    local cmd="$1"
    shift
    case "$cmd" in
        n|new)
            # Create the workspace (its path is printed), then jump into it.
            dir="$(wsjump new "$@")" || return 1
            [ -n "$dir" ] && cd -- "$dir"
            ;;
        t|touch)
            # Record a visit; with no path, the current directory.
            wsjump touch "$@"
            ;;
        s|sync)
            # Force a full re-read of every workspace's metadata.
            wsjump sync "$@"
            ;;
        -h|--help)
            printf '%s\n' \
                'usage:' \
                '  ju                 jump interactively' \
                '  ju n|new [slug...] create a workspace and jump into it' \
                '  ju t|touch [path]  record a visit (default: the current directory)' \
                '  ju s|sync [path]   force a full re-read of workspace metadata'
            ;;
        *)
            # An error, so render it on stderr: the success stream is read by
            # this function's callers for a path.
            printf "ju: unknown command '%s'\nrun 'ju -h' for usage\n" "$cmd" >&2
            return 1
            ;;
    esac
}"#;

/// Print the integration snippet for `shell` to stdout.
pub fn run(shell: Shell) -> AppResult<()> {
    let snippet = match shell {
        Shell::Bash => BASH_SNIPPET,
        Shell::Powershell => POWERSHELL_SNIPPET,
    };
    println!("{}", snippet);
    Ok(())
}
