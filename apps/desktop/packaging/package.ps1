# Build Nexus (the GPUI app) and package it with CEF for Windows (x86_64), as an installer:
# target\package\nexus-desktop-<version>-windows-x86_64.exe (packaging\windows\installer.nsi). CEF's DLLs
# and resources sit beside the exe, where Windows and src/web find them. Linux and macOS: packaging/package.sh.
#
#   pwsh packaging/package.ps1            build with the `dist` profile (Cargo.toml), then package it
#   $env:NO_BUILD = 1; pwsh packaging/package.ps1   package the exe already in target\dist
#
# It needs NSIS's makensis: on PATH, in its default folder, or installed here with Chocolatey.
#
# Signing, as the Nexus app's Windows build (the release workflow): with AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and
# AZURE_SIGNING_PROFILE set (and AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET for the CLI), Azure Trusted
# Signing's trusted-signing-cli signs the app's exe and the installer. Without them, both stay unsigned.
$ErrorActionPreference = 'Stop'
$crate = Split-Path -Parent $PSScriptRoot
Set-Location $crate
$bin = 'nexus-desktop-gpui'
# The release sets NEXUS_DESKTOP_BUILD_VERSION (an internal build's `<version>-internal.<n>`); else Cargo.toml's.
$version = if ($env:NEXUS_DESKTOP_BUILD_VERSION) { $env:NEXUS_DESKTOP_BUILD_VERSION } else {
  (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
}
$out = Join-Path $crate 'target\package'
$cefRoot = if ($env:CEF_PATH) { $env:CEF_PATH } else { Join-Path $crate 'target\cef' }
$buildProfile = if ($env:NEXUS_PROFILE) { $env:NEXUS_PROFILE } else { 'dist' }

if (-not $env:NO_BUILD) {
  # --timings writes target\cargo-timings\cargo-timing.html: each crate's compile time and the build's concurrency.
  # The release keeps it as an artifact. It changes nothing in the exe.
  cargo build --profile $buildProfile --locked --bin $bin --timings
  if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}
$exe = Join-Path $crate "target\$buildProfile\$bin.exe"
if (-not (Test-Path $exe)) { throw "no $exe; build first" }
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Path $out | Out-Null

$libcef = Get-ChildItem -Path $cefRoot -Recurse -Depth 4 -Filter libcef.dll | Select-Object -First 1
if (-not $libcef) { throw "no libcef.dll under $cefRoot" }
$name = "nexus-desktop-$version-windows-x86_64"
$stage = Join-Path $out $name
New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item $exe $stage

# CEF's files a running app needs. The rest is for building, or for the Chromium sandbox, which this build turns off.
# Of the locales, only English: see `locale` in src/web/host.rs.
$skip = @('include', 'libcef_dll', 'cmake', 'CMakeLists.txt', 'archive.json', 'chrome-sandbox', 'locales')
Get-ChildItem -Path $libcef.DirectoryName | Where-Object {
  ($skip -notcontains $_.Name) -and ($_.Extension -ne '.lib') -and ($_.Name -notlike 'bootstrap*.exe')
} | ForEach-Object { Copy-Item -Recurse $_.FullName $stage }
# Chromium also reads the gendered variants (`en-US_FEMININE.pak` and the others): without them every page's renderer
# stops.
$english = Get-ChildItem -Path $cefRoot -Recurse -Depth 5 -Filter 'en-US*.pak'
if (-not ($english | Where-Object Name -eq 'en-US.pak')) { throw "no locales\en-US.pak under $cefRoot" }
New-Item -ItemType Directory -Path (Join-Path $stage 'locales') | Out-Null
$english | ForEach-Object { Copy-Item $_.FullName (Join-Path $stage 'locales') }
# CEF's license (BSD-3-Clause) and Chromium's credits sit beside libcef.dll. CEF's archive keeps its license at its
# root, and download-cef does not keep that file, so packaging\ holds the text. The copy above takes CREDITS.html.
if (-not (Test-Path (Join-Path $stage 'CREDITS.html'))) {
  throw "no CREDITS.html beside $($libcef.FullName); a package must carry Chromium's credits"
}
Copy-Item (Join-Path $PSScriptRoot 'CEF-LICENSE.txt') $stage
# Nexus's own license (MIT), its NOTICE and the third-party notices of what the exe embeds sit beside CEF's.
# LICENSE and NOTICE are in oss\community here, and at the root of the public repository, two folders above the app.
$thirdParty = Join-Path $crate 'THIRD_PARTY_NOTICES.txt'
if (-not (Test-Path $thirdParty)) { throw "no THIRD_PARTY_NOTICES.txt in $crate; a package must carry it" }
Copy-Item $thirdParty $stage
foreach ($file in 'LICENSE', 'NOTICE') {
  $src = Join-Path $crate "oss\community\$file"
  if (-not (Test-Path $src)) { $src = Join-Path $crate "..\..\$file" }
  if (-not (Test-Path $src)) { throw "no $file in $crate\oss\community or $crate\..\..; a package must carry it" }
  Copy-Item $src (Join-Path $stage $file)
}

$signing = $env:AZURE_SIGNING_ENDPOINT -and $env:AZURE_SIGNING_ACCOUNT -and $env:AZURE_SIGNING_PROFILE
function Sign-File($file) {
  if (-not $signing) { return }
  trusted-signing-cli -e $env:AZURE_SIGNING_ENDPOINT -a $env:AZURE_SIGNING_ACCOUNT -c $env:AZURE_SIGNING_PROFILE -d 'Nexus' $file
  if ($LASTEXITCODE -ne 0) { throw "signing $file failed" }
}
Sign-File (Join-Path $stage "$bin.exe")

$makensis = Get-Command makensis -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source
if (-not $makensis) {
  $makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
  if (-not (Test-Path $makensis)) {
    choco install nsis -y --no-progress
    if ($LASTEXITCODE -ne 0) { throw "installing NSIS failed" }
  }
}
$setup = Join-Path $out "$name.exe"
# Nexus's icon, or the neutral stand-in where the marks are not in the tree (the open-source repo, assets/held/).
$icon = if (Test-Path 'assets\icons\icon.ico') { Resolve-Path 'assets\icons\icon.ico' } else { Resolve-Path 'assets\held\app-icon.ico' }
# The log shows how long the solid LZMA compression takes, apart from the build. Write-Host keeps the line
# out of this script's output, which is the installer's path.
$nsis = [Diagnostics.Stopwatch]::StartNew()
& $makensis /V2 "/DVERSION=$version" "/DSTAGE=$stage" "/DICON=$icon" "/DOUTFILE=$setup" 'packaging\windows\installer.nsi'
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }
Write-Host ("makensis took {0:N0} s" -f $nsis.Elapsed.TotalSeconds)
Sign-File $setup
Write-Output $setup
