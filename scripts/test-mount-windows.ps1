# End-to-end check of `pando mount` on Windows (ProjFS): serve a small repository, project
# it into an empty directory, and compare every file's hash with a normal checkout.
#   pwsh scripts/test-mount-windows.ps1 [-Bin target\release]
param([string]$Bin = "target\release")
$ErrorActionPreference = "Stop"
$Bin = (Resolve-Path $Bin).Path
$W = Join-Path ([IO.Path]::GetTempPath()) ("pando-mount-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force "$W\src\docs\deep\er", "$W\src\bin" | Out-Null
1..40 | ForEach-Object { [IO.File]::WriteAllText("$W\src\docs\f$_.txt", "line $_`n") }
[IO.File]::WriteAllText("$W\src\docs\deep\er\file.md", "nested`n")
$big = New-Object byte[] 12000000; (New-Object Random 7).NextBytes($big); [IO.File]::WriteAllBytes("$W\src\big.bin", $big)
git -C "$W\src" init -q; git -C "$W\src" -c core.autocrlf=false add -A; git -C "$W\src" -c user.name=t -c user.email=t@t commit -qm init

$server = Start-Process -PassThru -NoNewWindow -FilePath "$Bin\pandod.exe" `
  -ArgumentList "server", "--listen", "127.0.0.1:7499", "--metadata", "sqlite://$W/meta.db" -RedirectStandardError "$W\server.log"
Start-Sleep 3
& "$Bin\pando.exe" import-git "$W\src" --server http://127.0.0.1:7499 | Out-Null
& "$Bin\pando.exe" clone http://127.0.0.1:7499 "$W\clone" | Out-Null

$mount = Start-Process -PassThru -NoNewWindow -FilePath "$Bin\pando.exe" `
  -ArgumentList "mount", "$W\mnt", "--server", "http://127.0.0.1:7499" -RedirectStandardError "$W\mount.log"
Start-Sleep 5
if ($mount.HasExited) { Get-Content "$W\mount.log"; throw "pando mount exited" }

function Hashes($root) {
  Get-ChildItem $root -Recurse -File -Force | Where-Object { $_.FullName -notmatch '\\\.pando(\\|$)' } | ForEach-Object {
    "{0} {1}" -f $_.FullName.Substring($root.Length + 1), (Get-FileHash $_.FullName -Algorithm SHA256).Hash
  } | Sort-Object
}
try {
  $want = Hashes "$W\clone"; $got = Hashes "$W\mnt"
  $diff = Compare-Object $want $got
  if ($diff) { $diff | Format-Table | Out-String | Write-Host; throw "projection differs from checkout" }
  if ((Get-FileHash "$W\mnt\big.bin").Hash -ne (Get-FileHash "$W\src\big.bin").Hash) { throw "big.bin differs" }
  if (-not (Get-Item "$W\mnt\docs\f1.txt").IsReadOnly) { throw "projected files must be read-only" }
  $names = (Get-ChildItem "$W\mnt\docs" -Filter "f1*.txt").Name
  if ($names.Count -ne 11) { throw "search pattern returned $($names.Count) entries, expected 11" }
  if (-not (Test-Path "$W\mnt\DOCS\F2.TXT")) { throw "paths must be case-insensitive" }
  Write-Host "projection matches checkout: $($got.Count) files, read-only, pattern search and case-insensitive paths work"
} finally {
  Stop-Process -Id $mount.Id -Force -ErrorAction SilentlyContinue
  Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
}
