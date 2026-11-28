# Installs Nexus Desktop from the public stable channel. Run it in PowerShell:
#
#   irm https://nexus.inc/install.ps1 | iex
#
# macOS and Linux have their own script: curl -fsSL https://nexus.inc/install | sh
#
# It reads https://downloads.nexus.inc/desktop/stable/latest.json, downloads the Windows installer, checks its
# SHA-256 and runs it silently. The installer is per user, in %LOCALAPPDATA%\Programs\Nexus, and needs no
# administrator rights. Run this again to update. Then it opens the app.
#
# Setting, on the PowerShell side (a piped script takes no parameters):
#   $env:NEXUS_NO_LAUNCH = '1'   do not open the app at the end
#
# The whole script is one function, called on the last line, so a download cut short runs nothing.
# The build is not code-signed yet. A file that Invoke-WebRequest saves has no Mark of the Web,
# so SmartScreen does not stop this install. It would stop the same file saved by a browser.

$ManifestUrl = 'https://downloads.nexus.inc/desktop/stable/latest.json'
$FilePrefix = 'https://downloads.nexus.inc/desktop/stable/'
$DownloadKey = 'windows-x86_64-nsis'

# The manifest entry for this machine, or an error that says why there is none.
function Get-NexusDownload {
    param($Manifest, [string]$Key = $DownloadKey)
    $entry = $null
    if ($Manifest -and $Manifest.downloads) { $entry = $Manifest.downloads.PSObject.Properties[$Key] }
    if (-not $entry) { throw "Nexus $($Manifest.version) has no $Key download." }
    $item = $entry.Value
    if (-not $item.url -or -not $item.sha256) { throw "The $Key entry in the manifest has no url or sha256." }
    if (-not ([string]$item.url).StartsWith($FilePrefix)) { throw "The $Key download address is outside $FilePrefix." }
    if (([string]$item.sha256) -notmatch '^[0-9a-fA-F]{64}$') { throw "The $Key sha256 in the manifest is not a 64-character hex digest." }
    return $item
}

# Throws unless the file's SHA-256 is the wanted one.
function Assert-NexusChecksum {
    param([string]$Path, [string]$Expected)
    $got = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash
    if ($got -ne $Expected.ToUpperInvariant()) {
        throw "Checksum mismatch for $(Split-Path -Leaf $Path): expected $Expected, got $($got.ToLowerInvariant()). Nothing was installed."
    }
}

function Install-Nexus {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # Windows PowerShell 5.1 downloads far slower with its progress bar on.
    # Windows PowerShell 5.1 may default to TLS 1.0. [Net.SecurityProtocolType] exists on every version.
    try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }

    # A 32-bit PowerShell on 64-bit Windows reports x86 here and the real architecture in PROCESSOR_ARCHITEW6432.
    $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    if ($arch -ne 'AMD64') {
        throw "Nexus Desktop has a 64-bit x86 Windows build only (this machine reports $arch)."
    }

    $manifest = Invoke-RestMethod -Uri $ManifestUrl -UseBasicParsing
    if ($manifest -is [string]) { $manifest = $manifest | ConvertFrom-Json }
    if (-not $manifest.version) { throw "$ManifestUrl has no version." }
    Write-Host "Installing Nexus Desktop $($manifest.version)"
    $item = Get-NexusDownload -Manifest $manifest

    $dir = Join-Path ([IO.Path]::GetTempPath()) ('nexus-install-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir | Out-Null
    try {
        $file = Join-Path $dir ([IO.Path]::GetFileName(([Uri]$item.url).AbsolutePath))
        $mb = if ($item.size) { " ($([int]($item.size / 1MB)) MB)" } else { '' }
        Write-Host "Downloading $(Split-Path -Leaf $file)$mb"
        Invoke-WebRequest -Uri $item.url -OutFile $file -UseBasicParsing
        Assert-NexusChecksum -Path $file -Expected $item.sha256
        Write-Host 'Checksum ok'

        Write-Host 'Installing (a few seconds)'
        # /S is silent. The installer stops a running Nexus, replaces it, and keeps the user's data and sign-in.
        $run = Start-Process -FilePath $file -ArgumentList '/S' -Wait -PassThru
        if ($run.ExitCode -ne 0) { throw "The Nexus installer failed with exit code $($run.ExitCode)." }
    } finally {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }

    $app = Join-Path (Join-Path (Join-Path $env:LOCALAPPDATA 'Programs') 'Nexus') 'nexus-desktop-gpui.exe'
    if (-not (Test-Path -LiteralPath $app)) { throw "The installer finished, but $app is missing." }
    Write-Host "Installed $app"
    if ($env:NEXUS_NO_LAUNCH -eq '1') {
        Write-Host 'Start Nexus from the Start menu.'
    } else {
        Write-Host 'Opening Nexus'
        Start-Process -FilePath $app
    }
    Write-Host 'Sign in with your browser on first launch.'
}

Install-Nexus
