<#
.SYNOPSIS
    Automated Windows installer for Agent Workbench (Multi-Harness-ADE).

.DESCRIPTION
    Fetches the latest release from GitHub, downloads the installer,
    and runs the setup wizard.

.PARAMETER Tag
    Specific release tag to install (default: "latest").

.PARAMETER Silent
    Silent installation without GUI wizard prompts.

.PARAMETER DownloadOnly
    Download the installer to the current directory without running it.

.PARAMETER Portable
    Download the portable package instead of the setup installer (if available).

.EXAMPLE
    irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex
#>

[CmdletBinding()]
param(
    [string]$Tag = "latest",
    [switch]$Silent,
    [switch]$DownloadOnly,
    [switch]$Portable
)

$ErrorActionPreference = "Stop"

# Enable modern TLS protocols
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

$RepoOwner = "zkylek1212-k"
$RepoName = "Multi-Harness-ADE"

Write-Host ""
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "         Agent Workbench - Windows Quick Installer        " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host ""

$ApiUrl = if ($Tag -eq "latest") {
    "https://api.github.com/repos/$RepoOwner/$RepoName/releases/latest"
} else {
    "https://api.github.com/repos/$RepoOwner/$RepoName/releases/tags/$Tag"
}

Write-Host "Checking for latest release from GitHub..." -ForegroundColor Gray

$Headers = @{
    "User-Agent" = "AgentWorkbench-Installer"
    "Accept"     = "application/vnd.github.v3+json"
}

$Release = $null
try {
    $Release = Invoke-RestMethod -Uri $ApiUrl -Headers $Headers -Method Get
} catch {
    # Fallback to general releases list if /latest is not populated or redirects
    try {
        $Releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$RepoOwner/$RepoName/releases" -Headers $Headers -Method Get
        if ($Releases.Count -gt 0) {
            $Release = $Releases[0]
        }
    } catch {
        # ignore and handle below
    }
}

if (-not $Release) {
    Write-Host "[ERROR] Could not retrieve release info from GitHub API." -ForegroundColor Red
    Write-Host "Please download the installer directly from:" -ForegroundColor Yellow
    Write-Host "https://github.com/$RepoOwner/$RepoName/releases" -ForegroundColor Cyan
    exit 1
}

$ReleaseTag = $Release.tag_name
$ReleaseTitle = if ($Release.name) { $Release.name } else { $ReleaseTag }
Write-Host "Found Release: $ReleaseTitle ($ReleaseTag)" -ForegroundColor Green

# Locate appropriate asset
$Asset = $null
if ($Portable) {
    $Asset = $Release.assets | Where-Object { $_.name -like "*portable*" -or $_.name -like "*.zip" } | Select-Object -First 1
}

if (-not $Asset) {
    # Look for Windows setup installer (.exe, not blockmap, not portable)
    $Asset = $Release.assets | Where-Object {
        $_.name -like "*.exe" -and
        $_.name -notlike "*.blockmap" -and
        $_.name -notlike "*portable*"
    } | Select-Object -First 1
}

if (-not $Asset) {
    # Fallback: any .exe in assets
    $Asset = $Release.assets | Where-Object {
        $_.name -like "*.exe" -and $_.name -notlike "*.blockmap"
    } | Select-Object -First 1
}

if (-not $Asset) {
    Write-Host ""
    Write-Host "[NOTICE] Pre-built binary asset is not yet available in release $ReleaseTag." -ForegroundColor Yellow
    Write-Host "Please check the release page for updates: $($Release.html_url)" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Alternatively, you can run from source code:" -ForegroundColor Gray
    Write-Host "  git clone https://github.com/$RepoOwner/$RepoName.git" -ForegroundColor Gray
    Write-Host "  cd $RepoName" -ForegroundColor Gray
    Write-Host "  npm install" -ForegroundColor Gray
    Write-Host "  npm run dev" -ForegroundColor Gray
    exit 0
}

$FileName = $Asset.name
$DownloadUrl = $Asset.browser_download_url
$FileSizeMB = [math]::Round($Asset.size / 1MB, 2)

# Environment variable overrides (convenient when invoked via iex)
if ($env:INSTALL_SILENT -eq "1" -or $env:SILENT -eq "1") {
    $Silent = [switch]::Present
    $env:INSTALL_SILENT = $null
    $env:SILENT = $null
}
if ($env:INSTALL_DOWNLOAD_ONLY -eq "1" -or $env:DOWNLOAD_ONLY -eq "1") {
    $DownloadOnly = [switch]::Present
    $env:INSTALL_DOWNLOAD_ONLY = $null
    $env:DOWNLOAD_ONLY = $null
}

$DestDir = if ($DownloadOnly) { (Get-Location).Path } else { $env:TEMP }
$DestPath = Join-Path $DestDir $FileName

# If destination file already exists and is locked by an old abandoned process, use a unique name
try {
    if (Test-Path $DestPath) {
        $testStream = [System.IO.File]::Open($DestPath, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
        $testStream.Close()
        Remove-Item $DestPath -Force -ErrorAction SilentlyContinue
    }
} catch {
    $baseName = [System.IO.Path]::GetFileNameWithoutExtension($FileName)
    $ext = [System.IO.Path]::GetExtension($FileName)
    $randomTag = [System.IO.Path]::GetRandomFileName().Substring(0, 6)
    $DestPath = Join-Path $DestDir "$baseName-$randomTag$ext"
}


function Download-FileWithProgress {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$DestinationPath,
        [long]$ExpectedBytes = 0
    )

    # 1. Prefer curl.exe (standard in Windows 10/11) with live progress bar
    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        if (Test-Path $DestinationPath) { Remove-Item $DestinationPath -Force -ErrorAction SilentlyContinue }
        & curl.exe -fL --progress-bar --user-agent "AgentWorkbench-Installer" -o $DestinationPath $Url
        if ($LASTEXITCODE -eq 0 -and (Test-Path $DestinationPath)) {
            $actualLen = (Get-Item $DestinationPath).Length
            if ($ExpectedBytes -eq 0 -or $actualLen -ge ($ExpectedBytes * 0.99)) {
                return
            }
        }
        Write-Host "[Warning] curl.exe download was incomplete or failed. Falling back to .NET streaming..." -ForegroundColor Yellow
        if (Test-Path $DestinationPath) { Remove-Item $DestinationPath -Force -ErrorAction SilentlyContinue }
    }

    # 2. Fallback: .NET streaming download with live progress bar and guaranteed cleanup
    $res = $null
    $stream = $null
    $fileStream = $null
    try {
        $req = [System.Net.HttpWebRequest]::Create($Url)
        $req.AllowAutoRedirect = $true
        $req.UserAgent = "AgentWorkbench-Installer"
        $req.Timeout = 60000
        $res = $req.GetResponse()

        $totalBytes = if ($res.ContentLength -gt 0) { $res.ContentLength } else { $ExpectedBytes }
        $stream = $res.GetResponseStream()
        $fileStream = [System.IO.File]::Create($DestinationPath)
        $buffer = New-Object byte[] 65536
        $downloaded = [long]0
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $lastUpdate = [long]0

        while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $fileStream.Write($buffer, 0, $read)
            $downloaded += $read
            if ($sw.ElapsedMilliseconds - $lastUpdate -gt 250) {
                $lastUpdate = $sw.ElapsedMilliseconds
                $pct = if ($totalBytes -gt 0) { [math]::Min(100, [math]::Round(($downloaded / $totalBytes) * 100, 1)) } else { 0 }
                $mb = [math]::Round($downloaded / 1MB, 2)
                $totalMb = if ($totalBytes -gt 0) { [math]::Round($totalBytes / 1MB, 2) } else { "?" }
                Write-Progress -Activity "Downloading Agent Workbench" -Status "$pct% completed ($mb MB / $totalMb MB)" -PercentComplete $pct
                Write-Host -NoNewline "`rProgress: $pct% ($mb MB / $totalMb MB)  "
            }
        }
        Write-Progress -Activity "Downloading Agent Workbench" -Completed
        Write-Host "`rProgress: 100.0% ($([math]::Round($downloaded / 1MB, 2)) MB) - Completed!          " -ForegroundColor Green
        return
    } catch {
        Write-Host ""
        Write-Host "[Warning] Streaming download failed: $($_.Exception.Message). Falling back to Invoke-WebRequest..." -ForegroundColor Yellow
    } finally {
        if ($fileStream) { $fileStream.Dispose(); $fileStream = $null }
        if ($stream) { $stream.Dispose(); $stream = $null }
        if ($res) { $res.Dispose(); $res = $null }
    }

    # 3. Final fallback: Invoke-WebRequest
    try {
        if (Test-Path $DestinationPath) { Remove-Item $DestinationPath -Force -ErrorAction SilentlyContinue }
        Invoke-WebRequest -Uri $Url -OutFile $DestinationPath -UseBasicParsing -UserAgent "AgentWorkbench-Installer"
    } catch {
        Write-Host "[ERROR] All download attempts failed: $($_.Exception.Message)" -ForegroundColor Red
        exit 1
    }

    # Size validation
    if (Test-Path $DestinationPath) {
        $actual = (Get-Item $DestinationPath).Length
        if ($ExpectedBytes -gt 0 -and $actual -lt ($ExpectedBytes * 0.95)) {
            Write-Host "[ERROR] Downloaded file is incomplete ($([math]::Round($actual / 1MB, 2)) MB of $([math]::Round($ExpectedBytes / 1MB, 2)) MB)." -ForegroundColor Red
            Write-Host "Please download the installer directly from: https://github.com/$RepoOwner/$RepoName/releases/latest" -ForegroundColor Cyan
            exit 1
        }
    } else {
        Write-Host "[ERROR] Installer file was not saved." -ForegroundColor Red
        exit 1
    }
}

Write-Host "Downloading $FileName ($FileSizeMB MB)..." -ForegroundColor Cyan
Write-Host "From: $DownloadUrl" -ForegroundColor Gray

Download-FileWithProgress -Url $DownloadUrl -DestinationPath $DestPath -ExpectedBytes $Asset.size

Write-Host "Download completed successfully!" -ForegroundColor Green
Write-Host "Saved to: $DestPath" -ForegroundColor Gray

if ($DownloadOnly) {
    Write-Host ""
    Write-Host "[SUCCESS] Installer downloaded to $DestPath" -ForegroundColor Green
    Write-Host "You can execute it whenever you are ready." -ForegroundColor Cyan
    exit 0
}

# Run the installer
Write-Host ""
Write-Host "Launching installer..." -ForegroundColor Cyan

if ($FileName.EndsWith(".exe")) {
    $ProcessArgs = @{
        FilePath = $DestPath
    }
    if ($Silent) {
        $ProcessArgs["ArgumentList"] = "/S"
        Write-Host "Executing silent installation in background..." -ForegroundColor Cyan
        $proc = Start-Process @ProcessArgs -PassThru
        $spinner = @('|', '/', '-', '\')
        $sIdx = 0
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        while (-not $proc.HasExited) {
            $char = $spinner[$sIdx % $spinner.Length]
            $elapsedSec = [math]::Round($sw.Elapsed.TotalSeconds, 0)
            Write-Host -NoNewline "`rInstalling Agent Workbench... $char (${elapsedSec}s elapsed) "
            Start-Sleep -Milliseconds 250
            $sIdx++
        }
        $proc.WaitForExit()
        $elapsedTotal = [math]::Round($sw.Elapsed.TotalSeconds, 1)
        Write-Host "`rInstalling Agent Workbench... Done! (${elapsedTotal}s)                          " -ForegroundColor Green

        Write-Host ""
        if ($proc.ExitCode -eq 0) {
            Write-Host "[SUCCESS] Agent Workbench has been installed successfully!" -ForegroundColor Green
            $InstalledApp = Join-Path $env:LOCALAPPDATA "Agent Workbench\agent-workbench.exe"
            if (-not (Test-Path $InstalledApp)) {
                $InstalledApp = Join-Path $env:LOCALAPPDATA "Programs\Agent Workbench\Agent Workbench.exe"
            }
            if (Test-Path $InstalledApp) {
                Write-Host "Installed location: $InstalledApp" -ForegroundColor Gray
                Write-Host "Launching Agent Workbench..." -ForegroundColor Cyan
                Start-Process -FilePath $InstalledApp
            } else {
                Write-Host "You can start Agent Workbench from your Start Menu or Desktop shortcut." -ForegroundColor Cyan
            }
        } else {
            Write-Host "[WARNING] Installer exited with code $($proc.ExitCode)." -ForegroundColor Yellow
        }
    } else {
        $proc = Start-Process @ProcessArgs -PassThru
        Start-Sleep -Milliseconds 500
        if ($proc.HasExited -and $proc.ExitCode -ne 0) {
            Write-Host ""
            Write-Host "[ERROR] Installer failed to start or crashed (Exit code: $($proc.ExitCode))." -ForegroundColor Red
            Write-Host "You can try running the installer manually from: $DestPath" -ForegroundColor Yellow
            exit 1
        }
        Write-Host ""
        Write-Host "[SUCCESS] Setup wizard started. Follow the on-screen steps." -ForegroundColor Green
    }
} elseif ($FileName.EndsWith(".zip")) {
    $ExtractDir = Join-Path $env:LOCALAPPDATA "Programs\AgentWorkbench"
    Write-Host "Extracting portable files to $ExtractDir..." -ForegroundColor Cyan
    Expand-Archive -Path $DestPath -DestinationPath $ExtractDir -Force
    Write-Host "[SUCCESS] Extracted to $ExtractDir" -ForegroundColor Green
    $ExePath = Join-Path $ExtractDir "Agent Workbench.exe"
    if (Test-Path $ExePath) {
        Write-Host "Launching Agent Workbench..." -ForegroundColor Cyan
        Start-Process -FilePath $ExePath
    }
}
