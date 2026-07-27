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
# They were ADVISORY for the length of the Phase 1 reskin, because the
# pre-Nocturne code violated most of them by construction and a gate that always
# fails is a gate everyone learns to ignore. Every count reached zero with task
# 1.31, so as of 1.33 they are ENFORCED: one hit is a failure, and the shrinking
# baseline that used to live here is gone with the thing it was measuring.
# ---------------------------------------------------------------------------

$EnforceInvariants = $true

$gui = 'crates/vmiscope-gui/src'

function Forbid($id, $pattern, $why, $paths, $allow = $null) {
    $hits = @(Select-String -Path $paths -Pattern $pattern -ErrorAction SilentlyContinue)
    if ($allow) { $hits = @($hits | Where-Object { $_.Path -notmatch $allow }) }
    # Comment lines are prose, not code. These rules exist partly so the reason
    # for each ban can be written down next to the workaround, and a gate that
    # fires on its own explanation just teaches people not to explain.
    $hits = @($hits | Where-Object { $_.Line.TrimStart() -notmatch '^(//|/\*|\*)' })
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

    # The advisory path is kept for a future rule that lands against existing
    # violations: a new invariant can be introduced non-blocking, driven to zero,
    # and then enforced with the rest, without anyone having to rebuild this.
    Write-Host "==> $id advisory -- $n left ($why)" -ForegroundColor Yellow
    $hits | ForEach-Object {
        Write-Host ("    {0}:{1}: {2}" -f (Resolve-Path -Relative $_.Path), $_.LineNumber, $_.Line.Trim())
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

    # I9 has two halves because a glyph can enter the source two ways, and the
    # obvious grep only finds one of them. `default_fonts` is off, so anything
    # outside the allow-list below resolves in neither embedded text font and
    # renders as a blank box -- which nobody notices in a diff.
    #
    # Allow-list: typography that IS in Inter and JetBrains Mono (checked
    # against their cmap tables), not iconography.
    #   2014 em dash · 2026 ellipsis · 00B7 middle dot · 00D7 times
    #   201C/201D curly quotes · 2192 arrow · 25CF/25CB status dots
    $allowed = '2014|2026|00b7|00d7|201c|201d|2192|25cf|25cb'

    Forbid 'I9a' ('\\u\{(?!(' + $allowed + ')\})[0-9a-f]{4,5}\}') `
        'escaped glyph outside the typography allow-list; use theme::icons' `
        $allRs 'theme[\\/]icons\.rs'

    $rawHits = @()
    foreach ($f in $allRs) {
        if ($f -match 'theme[\\/]icons\.rs') { continue }
        $n = 0
        foreach ($line in [IO.File]::ReadAllLines($f)) {
            $n++
            foreach ($ch in $line.ToCharArray()) {
                $cp = [int][char]$ch
                if ($cp -gt 127 -and ('{0:x4}' -f $cp) -notmatch "^($allowed)$") {
                    $rawHits += "    {0}:{1}: U+{2:X4} '{3}'" -f (Resolve-Path -Relative $f), $n, $cp, $ch
                }
            }
        }
    }
    if ($rawHits) {
        Write-Host '==> I9b FAILED -- raw glyph pasted into source, outside the allow-list' -ForegroundColor Red
        $rawHits | Select-Object -Unique | ForEach-Object { Write-Host $_ }
        $script:failed += 'I9b'
    } else {
        Write-Host '==> I9b ok' -ForegroundColor DarkGray
    }
}

Write-Host ''
if ($failed.Count) {
    Write-Host ("FAILED: " + ($failed -join ', ')) -ForegroundColor Red
    exit 1
}
Write-Host 'All checks passed.' -ForegroundColor Green
