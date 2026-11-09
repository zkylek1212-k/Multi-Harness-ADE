<#
.SYNOPSIS
    Automated Release & Publish Workflow for Agent Workbench (Multi-Harness-ADE).
    (Maintainers only - requires repository write permissions via GitHub CLI 'gh')

.DESCRIPTION
    1. Validates GitHub CLI authentication (`gh`).
    2. Reads version from `package.json` (e.g. 0.1.3 -> v0.1.3).
    3. Runs TypeScript typecheck.
    4. Builds the installer with Tauri (npm run dist), signing it for the updater.
    5. Packages the program and its resources into a portable ZIP package.
    6. Creates or updates GitHub Release via `gh release` (supports assets up to 2GB).
    7. Uploads setup .exe, its signature, portable .zip, and latest.json updater metadata.
       The updater needs TAURI_SIGNING_PRIVATE_KEY (kept out of the repo) and WB_UPDATER_PUBKEY in the environment.
    8. Optionally copies assets to a local Google Drive folder if specified.

.PARAMETER Notes
    Optional release notes text or path to markdown notes. Defaults to latest git commit summary.

.PARAMETER SkipBuild
    Skip typecheck and `npm run dist` if build was already performed recently.

.PARAMETER Draft
    Publish as a GitHub draft release rather than immediately public.

.PARAMETER NoPublish
    Prepare the assets in release\ and stop before anything is uploaded.

.PARAMETER GoogleDrivePath
    Optional local directory path (e.g. "G:\My Drive\AgentWorkbench") to copy backups.

.EXAMPLE
    # Standard one-click release:
    npm run release

    # Release without rebuilding:
    powershell -ExecutionPolicy Bypass -File ./scripts/release.ps1 -SkipBuild
#>

[CmdletBinding()]
param(
    [string]$Notes = "",
    [switch]$SkipBuild,
    [switch]$Draft,
    [switch]$NoPublish,
    [string]$GoogleDrivePath = ""
)

$ErrorActionPreference = "Stop"

# Ensure modern TLS protocols
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

Write-Host ""
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "       Agent Workbench - Automated Release Publisher      " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host ""

# 0. Check GitHub CLI installation and authentication
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    if (Test-Path "C:\Program Files\GitHub CLI\gh.exe") {
        $env:PATH = "C:\Program Files\GitHub CLI;$env:PATH"
    } elseif (Test-Path "$env:LOCALAPPDATA\Programs\GitHub CLI\gh.exe") {
        $env:PATH = "$env:LOCALAPPDATA\Programs\GitHub CLI;$env:PATH"
    }
}

Write-Host "Checking GitHub CLI (gh) status..." -ForegroundColor Gray
try {
    $ghVersion = gh --version 2>&1 | Select-Object -First 1
    Write-Host "  -> Found: $ghVersion" -ForegroundColor Gray
} catch {
    Write-Host "[ERROR] GitHub CLI ('gh') is not found in PATH." -ForegroundColor Red
    Write-Host "Please install it from https://cli.github.com/ or run: winget install GitHub.cli" -ForegroundColor Yellow
    exit 1
}

$authCheck = gh auth status 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host "[ERROR] GitHub CLI is not logged in." -ForegroundColor Red
    Write-Host "Please run: gh auth login" -ForegroundColor Yellow
    exit 1
}
Write-Host "  -> GitHub authentication verified." -ForegroundColor Green

# 1. Read version from package.json
if (-not (Test-Path "package.json")) {
    Write-Host "[ERROR] package.json not found in current directory!" -ForegroundColor Red
    exit 1
}

$pkg = Get-Content -Raw "package.json" | ConvertFrom-Json
$version = $pkg.version
$tagName = "v$version"
$releaseTitle = "v$version"

Write-Host ""
Write-Host "Target Release Version: " -NoNewline -ForegroundColor White
Write-Host "$tagName" -ForegroundColor Cyan
Write-Host "Repository: zkylek1212-k/Multi-Harness-ADE" -ForegroundColor Gray
Write-Host ""

# A version already published from the Electron app must never be overwritten, in release\ or on GitHub
# No such release is the normal case here, and under Stop, Windows PowerShell treats gh's stderr as a fatal error
$ErrorActionPreference = "Continue"
$published = gh release view $tagName --json assets --jq '.assets[].name' 2>$null
$ErrorActionPreference = "Stop"
if ($published -contains "latest.yml") {
    Write-Host "[ERROR] $tagName is a published Electron release. Raise the version in package.json first." -ForegroundColor Red
    exit 1
}

# 2. Typecheck & Build
# The signing key lives outside the repo. These two can also come from the environment.
$defaultKey = Join-Path $env:USERPROFILE ".tauri\agent-workbench-updater.key"
if (-not $env:TAURI_SIGNING_PRIVATE_KEY -and (Test-Path $defaultKey)) { $env:TAURI_SIGNING_PRIVATE_KEY = $defaultKey }
if (-not $env:WB_UPDATER_PUBKEY -and (Test-Path "$defaultKey.pub")) { $env:WB_UPDATER_PUBKEY = (Get-Content -Raw "$defaultKey.pub").Trim() }
if (-not $env:TAURI_SIGNING_PRIVATE_KEY -or -not $env:WB_UPDATER_PUBKEY) {
    Write-Host "[ERROR] No updater signing key. Without a signature, installed apps cannot update." -ForegroundColor Red
    Write-Host "Create one with: npx tauri signer generate -w $defaultKey. Back up the private key outside the repo." -ForegroundColor Yellow
    exit 1
}

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $env:LOCALAPPDATA "wb-cargo-target" }
$env:CARGO_TARGET_DIR = $targetDir
$bundleDir = Join-Path $targetDir "release\bundle\nsis"

if (-not $SkipBuild) {
    Write-Host "[Step 1/4] Running TypeScript Typecheck..." -ForegroundColor Cyan
    npm run typecheck
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[ERROR] TypeScript typecheck failed! Aborting release." -ForegroundColor Red
        exit 1
    }
    Write-Host "[Step 2/4] Building the installer..." -ForegroundColor Cyan
    npm run build:ui
    if ($LASTEXITCODE -ne 0) { exit 1 }
    # Signing while bundling needs the public key in the config too
    $updaterConfig = Join-Path $env:TEMP "agent-workbench-updater-config.json"
    $configJson = @{
        bundle = @{ createUpdaterArtifacts = $true }
        plugins = @{ updater = @{ pubkey = $env:WB_UPDATER_PUBKEY; endpoints = @("https://github.com/zkylek1212-k/Multi-Harness-ADE/releases/latest/download/latest.json") } }
    } | ConvertTo-Json -Depth 5
    [System.IO.File]::WriteAllText($updaterConfig, $configJson, (New-Object System.Text.UTF8Encoding $false))
    npx tauri build --ci --config $updaterConfig
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[ERROR] Build failed! Aborting release." -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host "[Notice] -SkipBuild specified: Skipping typecheck and build." -ForegroundColor Yellow
}

# 3. Assets under the names install.ps1 and the updater expect
New-Item -ItemType Directory -Force -Path "release" | Out-Null
$built = Get-ChildItem $bundleDir -Filter "*$version*-setup.exe" | Select-Object -First 1
if (-not $built) {
    Write-Host "[ERROR] No installer for $version in $bundleDir." -ForegroundColor Red
    exit 1
}
$setupName = "Agent-Workbench-$version-setup.exe"
$setupPath = Join-Path "release" $setupName
Copy-Item $built.FullName $setupPath -Force
$signature = Get-Content -Raw "$($built.FullName).sig"
Copy-Item "$($built.FullName).sig" "$setupPath.sig" -Force

$latest = @{
    version = $version
    pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = @{ "windows-x86_64" = @{ signature = $signature.Trim(); url = "https://github.com/zkylek1212-k/Multi-Harness-ADE/releases/download/$tagName/$setupName" } }
}
$latestJson = "release\latest.json"
[System.IO.File]::WriteAllText($latestJson, ($latest | ConvertTo-Json -Depth 5), (New-Object System.Text.UTF8Encoding $false))

$portableZip = "release\Agent-Workbench-$version-portable.zip"
$portableDir = Join-Path $env:TEMP "agent-workbench-portable-$version"
Remove-Item $portableDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $portableDir | Out-Null
Copy-Item (Join-Path $targetDir "release\agent-workbench.exe") $portableDir
Copy-Item (Join-Path $targetDir "release\plugins") $portableDir -Recurse
Compress-Archive -Path "$portableDir\*" -DestinationPath $portableZip -Force
Remove-Item $portableDir -Recurse -Force -ErrorAction SilentlyContinue

Write-Host "[Step 4/4] Publishing to GitHub Releases ($tagName)..." -ForegroundColor Cyan
$uploadFiles = @((Resolve-Path $setupPath).Path, (Resolve-Path "$setupPath.sig").Path, (Resolve-Path $latestJson).Path, (Resolve-Path $portableZip).Path)

Write-Host "Assets prepared for upload:" -ForegroundColor White
foreach ($f in $uploadFiles) {
    $item = Get-Item $f
    $mb = [math]::Round($item.Length / 1MB, 2)
    Write-Host "  * $($item.Name) ($mb MB)" -ForegroundColor Cyan
}

if ($NoPublish) {
    Write-Host "[Notice] -NoPublish specified: assets are in release\, nothing was uploaded." -ForegroundColor Yellow
    exit 0
}

# Determine release notes
$tempNotesFile = Join-Path $env:TEMP "agent_workbench_release_notes_$tagName.md"
if ($Notes -and (Test-Path $Notes -ErrorAction SilentlyContinue)) {
    $tempNotesFile = (Resolve-Path $Notes).Path
} else {
    $releaseNotes = $Notes
    if (-not $releaseNotes) {
        try {
            $recentCommits = git log -n 5 --oneline 2>&1
            $releaseNotes = "### Changes in $tagName`n`n" + ($recentCommits -join "`n")
        } catch {
            $releaseNotes = "Release $tagName of Agent Workbench."
        }
    }
    [System.IO.File]::WriteAllText($tempNotesFile, $releaseNotes, [System.Text.Encoding]::UTF8)
}

# Check if release tag already exists on GitHub
$releaseExists = $false
try {
    $viewOut = gh release view $tagName --json tagName 2>&1
    if ($LASTEXITCODE -eq 0) {
        $releaseExists = $true
    }
} catch {
    $releaseExists = $false
}

if ($releaseExists) {
    Write-Host ""
    Write-Host "Release $tagName already exists on GitHub. Uploading and updating assets..." -ForegroundColor Yellow
    gh release upload $tagName $uploadFiles --clobber
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[ERROR] Failed to upload assets to GitHub Release $tagName." -ForegroundColor Red
        exit 1
    }
} else {
    Write-Host ""
    Write-Host "Creating new GitHub Release $tagName..." -ForegroundColor Green
    $createArgs = @(
        "release", "create", $tagName,
        "--title", $releaseTitle,
        "--notes-file", $tempNotesFile
    )
    if ($Draft) {
        $createArgs += "--draft"
    }
    foreach ($f in $uploadFiles) {
        $createArgs += $f
    }
    & gh @createArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[ERROR] Failed to create GitHub Release $tagName." -ForegroundColor Red
        exit 1
    }
}

# 5. Optional Google Drive backup copy
if ($GoogleDrivePath) {
    if (Test-Path $GoogleDrivePath) {
        Write-Host ""
        Write-Host "Copying assets to local Google Drive backup folder..." -ForegroundColor Cyan
        foreach ($f in $uploadFiles) {
            Copy-Item -Path $f -Destination $GoogleDrivePath -Force
            Write-Host "  -> Copied $(Split-Path $f -Leaf) to $GoogleDrivePath" -ForegroundColor Green
        }
    } else {
        Write-Host "[Warning] GoogleDrivePath '$GoogleDrivePath' was not found. Skipped local copy." -ForegroundColor Yellow
    }
}

# Summary output
Write-Host ""
Write-Host "==========================================================" -ForegroundColor Green
Write-Host "      Release $tagName Successfully Published!         " -ForegroundColor Green
Write-Host "==========================================================" -ForegroundColor Green
Write-Host ""
Write-Host "Release Page URL:" -ForegroundColor White
Write-Host "  https://github.com/zkylek1212-k/Multi-Harness-ADE/releases/tag/$tagName" -ForegroundColor Cyan
Write-Host ""
Write-Host "Auto-Updater:" -ForegroundColor White
Write-Host "  latest.json uploaded; installed apps verify the signature and offer the update." -ForegroundColor Gray
Write-Host ""
Write-Host "Quick Install Command for Users:" -ForegroundColor White
Write-Host "  irm https://raw.githubusercontent.com/zkylek1212-k/Multi-Harness-ADE/master/install.ps1 | iex" -ForegroundColor Yellow
Write-Host ""
