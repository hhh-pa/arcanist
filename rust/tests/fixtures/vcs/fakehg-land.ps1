# Fake `hg` for `arc land` seam tests (L10). Same protocol as
# fakehg-land.sh: logs argv to .fakehg.log, answers from .fakehg-*
# marker files, honors .fakehg-fail-<sub> failure markers.
# The .cmd forwarder quotes a bare `-` argv (from `--logfile -`) as `"-"`
# so -File binding accepts it; unwrap it back so the log shows true argv.
$argv = @($args | ForEach-Object { if ($_ -eq '"-"') { '-' } else { $_ } })
$raw = $argv -join ' '
Add-Content -Path .\.fakehg.log -Value $raw -Encoding Ascii
if ($argv.Count -ge 2 -and $argv[0] -eq '--encoding') {
  if ($argv.Count -gt 2) { $argv = $argv[2..($argv.Count - 1)] } else { $argv = @() }
}
if ($argv.Count -ge 2 -and $argv[0] -eq '--config') {
  if ($argv.Count -gt 2) { $argv = $argv[2..($argv.Count - 1)] } else { $argv = @() }
}
$sub = ''
if ($argv.Count -gt 0 -and $null -ne $argv[0]) { $sub = [string]$argv[0] }
if ($sub -eq '') { exit 0 }
if (Test-Path ".fakehg-fail-$sub") { exit 1 }
$NODE = '0123456789abcdef0123456789abcdef01234567'
$TIP = '89abcdef0123456789abcdef0123456789abcdef01'
function Emit([string]$name) {
  if (Test-Path $name) {
    $bytes = [System.IO.File]::ReadAllBytes($name)
    [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
  }
}
function EmitOr([string]$name, [string]$text) {
  if (Test-Path $name) { Emit $name }
  else {
    $bytes = [System.Text.Encoding]::ASCII.GetBytes($text)
    [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
  }
}
function EmitText([string]$text) {
  $bytes = [System.Text.Encoding]::ASCII.GetBytes($text)
  [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
}
switch ($sub) {
  'branch' { EmitText "default`n"; break }
  'branches' { Emit '.fakehg-branches'; break }
  'bookmarks' {
    if (Test-Path .fakehg-bookmarks) { Emit '.fakehg-bookmarks' }
    else { EmitText " * feature  4:aaaaaaaabbbbccccddddeeeeffff0000111122223333`n" }
    break
  }
  'status' { Emit '.fakehg-status'; break }
  'paths' { if (-not (Test-Path .fakehg-nopaths)) { EmitText "https://hg.example.test/repo`n" }; break }
  'identify' { EmitText "$NODE`n"; break }
  'log' {
    if ($raw -like '*parents %*') { Emit '.fakehg-log-commits' }
    elseif ($raw -like '*-r .') { EmitOr '.fakehg-node' $NODE }
    elseif ($raw -like '*--rev tip*') { EmitOr '.fakehg-tip' $TIP }
    elseif ($raw -like '*{desc}*') {
      if (Test-Path .fakehg-msg) { Emit '.fakehg-msg' } else { EmitText "old message`n" }
    }
    elseif ($raw -like '*children(*') { Emit '.fakehg-children' }
    elseif ($raw -like '*nope*') { exit 1 }
    else { EmitText $NODE }
    break
  }
  'rebase' {
    if ([Console]::IsInputRedirected) { foreach ($line in $input) { } }
    break
  }
  default { break }
}
exit 0
