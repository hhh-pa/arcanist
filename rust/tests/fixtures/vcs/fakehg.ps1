# Fake `hg` for local-commit-info tests. cmd.exe cannot receive the
# `{date|rfc822date}` pipe in template argv, so these tests run through
# PowerShell, which preserves every byte. Same files, markers, and log as
# fakehg.bat.
$raw = $args -join ' '
Add-Content -Path .\.fakehg.log -Value $raw -Encoding Ascii
$argv = @($args)
if ($argv.Count -ge 2 -and $argv[0] -eq '--encoding') {
  if ($argv.Count -gt 2) { $argv = $argv[2..($argv.Count - 1)] } else { $argv = @() }
}
$sub = ''
if ($argv.Count -gt 0 -and $null -ne $argv[0]) { $sub = [string]$argv[0] }
if ($sub -eq '') { exit 0 }
if (Test-Path ".fakehg-fail-$sub") { exit 1 }
$NODE = '0123456789abcdef0123456789abcdef01234567'
function Emit([string]$name) {
  if (Test-Path $name) {
    $bytes = [System.IO.File]::ReadAllBytes($name)
    [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
  }
}
function EmitText([string]$text) {
  $bytes = [System.Text.Encoding]::ASCII.GetBytes($text)
  [Console]::OpenStandardOutput().Write($bytes, 0, $bytes.Length)
}
switch ($sub) {
  'branch' { EmitText "default`n"; break }
  'status' { Emit '.fakehg-status'; break }
  'log' {
    if ($raw -like '*children(.)*') { Emit '.fakehg-children' }
    elseif ($raw -like '*{rev}*') { Emit '.fakehg-log-info' }
    elseif ($raw -like '*nope*') { exit 1 }
    elseif ($raw -like '*::. -*') { Emit '.fakehg-log-msg' }
    elseif ($raw -like '*::.*') { Emit '.fakehg-log-nodiff' }
    elseif ($raw -like '*--branch*') { Emit '.fakehg-log-draft' }
    elseif ($raw -like '*outgoing()*') { EmitText $NODE }
    elseif ($raw -like '*bookmark()*') { EmitText $NODE }
    elseif ($raw -like '*parents(draft())*') { Emit '.fakehg-log-pub' }
    elseif ($raw -like '*ancestor(*') { EmitText $NODE }
    elseif ($raw -like '*{desc}*') {
      if (Test-Path .fakehg-msg) { Emit '.fakehg-msg' } else { EmitText "old message`n" }
    }
    else { EmitText $NODE }
    break
  }
  'parents' {
    if ($raw -like '*{node}*') { Emit '.fakehg-parents-tpl' } else { Emit '.fakehg-parents' }
    break
  }
  'manifest' { Emit '.fakehg-manifest'; break }
  'annotate' { Emit '.fakehg-annotate'; break }
  'cat' {
    if ($raw -like '*--output*') { }
    elseif ($raw -like '*absent*') { exit 1 }
    else { Emit '.fakehg-cat' }
    break
  }
  'showconfig' { EmitText "Test User <t@example.test>`n"; break }
  'help' {
    if (Test-Path .fakehg-help) { Emit '.fakehg-help' }
    elseif (Test-Path .fakehg-noamend) { EmitText "no flags here`n" }
    else { EmitText "hg commit [OPTION]...`n  --amend flag`n" }
    break
  }
  'paths' { if (-not (Test-Path .fakehg-nopaths)) { EmitText "https://hg.example.test/repo`n" }; break }
  'svn' { Emit '.fakehg-svninfo'; break }
  'diff' { Emit '.fakehg-diff'; break }
  'envprobe' { EmitText "HGPLAIN=$env:HGPLAIN`n"; break }
  default { break }
}
exit 0
