# DBCrust Windows Install Script
# PowerShell script to install DBCrust on Windows systems

param(
    [string]$Version = "latest",
    [string]$InstallDir = "",
    [switch]$NoModifyPath,
    [switch]$Quiet,
    [switch]$Verbose,
    [switch]$Help
)

$ErrorActionPreference = "Stop"

# Configuration
$APP_NAME = "dbcrust"
$GITHUB_REPO = "clement-tourriere/dbcrust"
$INSTALLER_BASE_URL = if ($env:DBCRUST_INSTALLER_GITHUB_BASE_URL) { $env:DBCRUST_INSTALLER_GITHUB_BASE_URL } else { "https://github.com" }

if ($env:DBCRUST_VERSION) { $Version = $env:DBCRUST_VERSION }
if ($env:DBCRUST_INSTALL_DIR) { $InstallDir = $env:DBCRUST_INSTALL_DIR }
if ($env:DBCRUST_NO_MODIFY_PATH -eq "1") { $NoModifyPath = $true }

$ARTIFACT_DOWNLOAD_URL = if ($Version -eq "latest") {
    "$INSTALLER_BASE_URL/$GITHUB_REPO/releases/latest/download"
} else {
    "$INSTALLER_BASE_URL/$GITHUB_REPO/releases/download/$Version"
}

function Show-Usage {
    Write-Host @"
DBCrust Windows Installer

This script installs DBCrust binaries on Windows systems.

Downloads appropriate archive from:
$INSTALLER_BASE_URL/$GITHUB_REPO/releases/

USAGE:
    .\install.ps1 [OPTIONS]

OPTIONS:
    -Version <version>      Specify the version to install (default: latest)
    -InstallDir <path>      Override the installation directory
    -NoModifyPath          Don't modify PATH environment variable
    -Quiet                 Disable progress output
    -Verbose               Enable verbose output
    -Help                  Show this help message

ENVIRONMENT VARIABLES:
    DBCRUST_VERSION        Specify the version to install
    DBCRUST_INSTALL_DIR    Override the installation directory
    DBCRUST_NO_MODIFY_PATH Don't modify PATH (set to 1)

Examples:
    .\install.ps1
    .\install.ps1 -Version v0.12.2
    .\install.ps1 -InstallDir "C:\Tools\dbcrust" -NoModifyPath
"@
}

function Write-Info {
    param([string]$Message)
    if (-not $Quiet) {
        Write-Host $Message -ForegroundColor Green
    }
}

function Write-Verbose-Info {
    param([string]$Message)
    if ($Verbose) {
        Write-Host $Message -ForegroundColor Cyan
    }
}

function Write-Error-Info {
    param([string]$Message)
    if (-not $Quiet) {
        Write-Host "ERROR: $Message" -ForegroundColor Red
    }
    exit 1
}

function Get-Architecture {
    $arch = $env:PROCESSOR_ARCHITECTURE
    switch ($arch) {
        "AMD64" { return "x86_64-pc-windows-msvc" }
        "x86" { return "i686-pc-windows-msvc" }
        default { Write-Error-Info "Unsupported architecture: $arch" }
    }
}

function Get-InstallDirectory {
    if ($InstallDir) {
        return $InstallDir
    }
    
    # Try standard Windows locations
    $localAppData = $env:LOCALAPPDATA
    if ($localAppData) {
        return Join-Path $localAppData "Programs\dbcrust"
    }
    
    # Fallback to user profile
    $userProfile = $env:USERPROFILE
    if ($userProfile) {
        return Join-Path $userProfile ".local\bin"
    }
    
    Write-Error-Info "Could not determine installation directory"
}

function Download-And-Install {
    $architecture = Get-Architecture
    Write-Info "Detected platform: $architecture"
    
    $archiveName = "dbcrust-$architecture.zip"
    $downloadUrl = "$ARTIFACT_DOWNLOAD_URL/$archiveName"
    
    $tempDir = New-TemporaryFile | ForEach-Object { Remove-Item $_; New-Item -ItemType Directory -Path $_ }
    $archiveFile = Join-Path $tempDir $archiveName
    
    Write-Info "Downloading DBCrust $architecture"
    Write-Verbose-Info "  from $downloadUrl"
    Write-Verbose-Info "  to $archiveFile"
    
    try {
        Invoke-WebRequest -Uri $downloadUrl -OutFile $archiveFile -UseBasicParsing
    }
    catch {
        Write-Error-Info "Failed to download $downloadUrl. This may be a network error, or the release may not have binaries for your platform. Please check $INSTALLER_BASE_URL/$GITHUB_REPO/releases for available downloads."
    }
    
    # Extract archive
    Write-Verbose-Info "Extracting archive to $tempDir"
    Expand-Archive -Path $archiveFile -DestinationPath $tempDir -Force
    
    # Install binaries
    Install-Binaries -SourceDir $tempDir -Binaries @("dbcrust.exe", "dbc.exe")
    
    # Cleanup
    Remove-Item $tempDir -Recurse -Force
}

function Install-Binaries {
    param(
        [string]$SourceDir,
        [string[]]$Binaries
    )
    
    $installDir = Get-InstallDirectory
    Write-Info "Installing to $installDir"
    
    # Create install directory
    if (-not (Test-Path $installDir)) {
        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    }
    
    # Copy binaries
    foreach ($binary in $Binaries) {
        $sourcePath = Join-Path $SourceDir $binary
        if (Test-Path $sourcePath) {
            $destPath = Join-Path $installDir $binary
            Copy-Item $sourcePath $destPath -Force
            Write-Info "  $binary"
        } else {
            Write-Verbose-Info "  $binary not found in archive (skipping)"
        }
    }
    
    Write-Info "Installation complete!"
    
    # Check if install dir is already in PATH
    $currentPath = [Environment]::GetEnvironmentVariable("PATH", [EnvironmentVariableTarget]::User)
    if ($currentPath -and $currentPath.Split(';') -contains $installDir) {
        $NoModifyPath = $true
        Write-Info "$installDir is already in PATH"
    }
    
    # Configure PATH
    if (-not $NoModifyPath) {
        Add-To-Path -InstallDir $installDir
    }
}

function Add-To-Path {
    param([string]$InstallDir)
    
    try {
        $currentPath = [Environment]::GetEnvironmentVariable("PATH", [EnvironmentVariableTarget]::User)
        if (-not $currentPath) {
            $currentPath = ""
        }
        
        $newPath = if ($currentPath) {
            "$InstallDir;$currentPath"
        } else {
            $InstallDir
        }
        
        [Environment]::SetEnvironmentVariable("PATH", $newPath, [EnvironmentVariableTarget]::User)
        
        Write-Info ""
        Write-Info "DBCrust has been added to your PATH."
        Write-Info "Please restart your terminal or run the following to update your current session:"
        Write-Info ""
        Write-Info "  `$env:PATH = `"$InstallDir;`$env:PATH`""
        Write-Info ""
    }
    catch {
        Write-Info ""
        Write-Info "Could not automatically add DBCrust to PATH."
        Write-Info "Please add the following directory to your PATH manually:"
        Write-Info ""
        Write-Info "  $InstallDir"
        Write-Info ""
    }
}

# Main execution
if ($Help) {
    Show-Usage
    exit 0
}

try {
    Download-And-Install
}
catch {
    Write-Error-Info $_.Exception.Message
}