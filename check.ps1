# Local gate. Runs what CI runs, plus the design-system invariants that a
# compiler cannot enforce. The pre-push hook calls the cargo half of this; run
# the whole thing before opening a pull request.
#
#   .\check.ps1            # everything
#   .\check.ps1 -SkipCargo # invariants only (fast)

param([switch]$SkipCargo)

$ErrorActionPreference = 'Stop'
$failed = @()

function Step($name, $block) {
    Write-Host "==> $name" -ForegroundColor Cyan
    & $block
    if ($LASTEXITCODE -ne 0) { $script:failed += $name }
}

if (-not $SkipCargo) {
    Step 'cargo fmt --check'  { cargo fmt --all --check }
    Step 'cargo clippy'       { cargo clippy --all-targets -- -D warnings }
    Step 'cargo test'         { cargo test --all }
    Step 'cargo build'        { cargo build --all }
}

# ---------------------------------------------------------------------------
# Design-system invariants (docs/REDESIGN.md, "Standing invariants").
#
# These are greps rather than lints because none of them is a compile error --
# every one of them is something that builds fine and quietly breaks the look,
# which is exactly the class of mistake a reskin invites.
#
# They are ADVISORY until the Phase 1 reskin lands, because the pre-Nocturne
# code violates most of them by construction and a gate that always fails is a
# gate everyone learns to ignore. The counts below are the Phase 0 baseline:
# each one must reach zero during Phase 1, at which point flip $EnforceInvariants
# to $true (task 1.33) and the numbers stop being a target and start being a
# floor.
# ---------------------------------------------------------------------------

$EnforceInvariants = $false
$Baseline = @{ I1 = 29; I2 = 27; I3 = 3; I4 = 0; I5 = 0; I6 = 0 }

$gui = 'crates/vmiscope-gui/src'

function Forbid($id, $pattern, $why, $paths, $allow = $null) {
    $hits = @(Select-String -Path $paths -Pattern $pattern -ErrorAction SilentlyContinue)
    if ($allow) { $hits = @($hits | Where-Object { $_.Path -notmatch $allow }) }
    $n = $hits.Count

    if ($n -eq 0) {
        Write-Host "==> $id ok" -ForegroundColor DarkGray
        return
    }

    if ($script:EnforceInvariants) {
        Write-Host "==> $id FAILED -- $why" -ForegroundColor Red
        $hits | ForEach-Object {
            Write-Host ("    {0}:{1}: {2}" -f (Resolve-Path -Relative $_.Path), $_.LineNumber, $_.Line.Trim())
        }
        $script:failed += $id
        return
    }

    # Advisory mode: silent while shrinking, loud if it grows.
    $was = $script:Baseline[$id]
    if ($n -gt $was) {
        Write-Host "==> $id REGRESSED -- $n occurrences, was $was ($why)" -ForegroundColor Red
        $hits | ForEach-Object {
            Write-Host ("    {0}:{1}: {2}" -f (Resolve-Path -Relative $_.Path), $_.LineNumber, $_.Line.Trim())
        }
        $script:failed += "$id (regression)"
    } else {
        $note = if ($n -lt $was) { "down from $was" } else { "at baseline" }
        Write-Host "==> $id advisory -- $n left, $note" -ForegroundColor Yellow
    }
}

$allRs = Get-ChildItem -Path $gui -Recurse -Filter *.rs -ErrorAction SilentlyContinue |
         Select-Object -ExpandProperty FullName

if ($allRs) {
    Forbid 'I1' 'Color32::from_rgb|Color32::from_gray' `
        'colours belong in theme/tokens.rs, not in a view' `
        $allRs 'theme[\\/]tokens\.rs'

    Forbid 'I2' 'ui\.separator\(\)' `
        'separator_style hard-codes 6.0 spacing; use widgets::rule' `
        $allRs

    Forbid 'I3' 'RichText::new\([^)]*\)\.strong\(\)|\.strong\(\)' `
        'strong() recolours rather than emboldens; use the ui-med family' `
        $allRs

    Forbid 'I4' 'Column::auto\(\)' `
        'auto() widths jitter inside a virtualised table' `
        $allRs

    Forbid 'I5' 'Context::set_style|ctx\.set_style|ctx\.style\(\)' `
        'removed in egui 0.35; use all_styles_mut / global_style' `
        $allRs

    Forbid 'I6' 'SidePanel|TopBottomPanel|popup_below_widget|menu::bar|screen_rect\(\)|Color32::lerp' `
        'removed in egui 0.35' `
        $allRs
}

Write-Host ''
if ($failed.Count) {
    Write-Host ("FAILED: " + ($failed -join ', ')) -ForegroundColor Red
    exit 1
}
Write-Host 'All checks passed.' -ForegroundColor Green
