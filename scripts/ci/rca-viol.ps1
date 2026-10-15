# Local mirror of scripts/ci/go/cmd/complexity for Windows dev machines
# without Python: reports functions with cyclomatic>15 or cognitive>20.
# Usage: powershell -ExecutionPolicy Bypass -File scripts/ci/rca-viol.ps1 -Ws <rust-dir>
# CI remains authoritative; thresholds must match cmd/complexity.
param([string]$Ws = '')
if (-not $Ws) { $Ws = Join-Path (Split-Path (Split-Path $PSScriptRoot -Parent) -Parent) 'rust' }
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Web.Extensions
$js = New-Object Web.Script.Serialization.JavaScriptSerializer
$js.MaxJsonLength = [int]::MaxValue
function Walk($spaces, $stack, $path) {
    foreach ($s in @($spaces)) {
        $kind = $s['kind']; $name = $s['name']
        if ($kind -eq 'function' -and $name -ne '<anonymous>') {
            $cy = 0; $co = 0
            try { $cy = [double]$s['metrics']['cyclomatic']['sum'] } catch {}
            try { $co = [double]$s['metrics']['cognitive']['sum'] } catch {}
            if ($cy -gt 15 -or $co -gt 20) {
                [pscustomobject]@{ File = $path; Fn = (($stack + @($name)) -join '::'); Cy = $cy; Co = $co }
            }
            Walk $s['spaces'] ($stack + @($name)) $path
        } elseif ($kind -eq 'closure') {
            Walk $s['spaces'] $stack $path
        } else {
            $extra = @()
            if ($name -and $name -ne '?') { $extra = @($name) }
            Walk $s['spaces'] ($stack + $extra) $path
        }
    }
}
$outdir = Join-Path ([IO.Path]::GetTempPath()) ('rca-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $outdir | Out-Null
Push-Location $Ws
try {
    $rss = Get-ChildItem $Ws -Recurse -Filter '*.rs' |
        Where-Object { $_.FullName -notmatch '\\target' } |
        ForEach-Object { './' + $_.FullName.Substring($Ws.Length + 1).Replace('\', '/') } |
        Sort-Object
    $args = @('-m', '-O', 'json', '-o', $outdir)
    foreach ($f in $rss) { $args += @('-p', $f) }
    & rust-code-analysis-cli @args | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "rca exit $LASTEXITCODE" }
    $viol = @()
    Get-ChildItem $outdir -Recurse -Filter '*.json' | ForEach-Object {
        $rel = $_.FullName.Substring($outdir.Length + 1).Replace('\', '/') -replace '\.json$', ''
        if ($rel.StartsWith('./')) { $rel = $rel.Substring(2) }
        $d = $js.DeserializeObject((Get-Content $_.FullName -Raw))
        $viol += @(Walk $d['spaces'] @() $rel)
    }
    $viol | Sort-Object File, Fn | Format-Table -AutoSize | Out-String | Write-Host
    Write-Host ("TOTAL: {0} violations" -f $viol.Count)
} finally {
    Pop-Location
    Remove-Item $outdir -Recurse -Force -ErrorAction SilentlyContinue
}
