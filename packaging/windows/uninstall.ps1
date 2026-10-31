#Requires -Version 5.1
<#
.SYNOPSIS
    Removes the current-user picasa-rs file associations.

.DESCRIPTION
    Clears picasa-rs associations, removes the ProgID and App Paths entries, and
    optionally deletes the installed executable. Other applications'
    associations are left untouched.

.PARAMETER InstallDir
    Directory used by install.ps1. Defaults to %LOCALAPPDATA%\Programs\picasa-rs.

.PARAMETER Extensions
    File extensions to clean up. Defaults to the formats install.ps1 registers.

.PARAMETER RemoveFiles
    Also delete the install directory.

.EXAMPLE
    .\uninstall.ps1

.EXAMPLE
    .\uninstall.ps1 -RemoveFiles
#>
[CmdletBinding()]
param(
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\picasa-rs'),
    [string[]]$Extensions = @(
        '.jpg', '.jpeg', '.jfif', '.png', '.webp',
        '.tif', '.tiff', '.bmp', '.gif'
    ),
    [switch]$RemoveFiles
)

$ErrorActionPreference = 'Stop'

$progId = 'PicasaRs.Image'
$classes = 'HKCU:\Software\Classes'

foreach ($extension in $Extensions) {
    if (-not $extension.StartsWith('.')) {
        $extension = ".$extension"
    }
    $extension = $extension.ToLowerInvariant()
    $extKey = Join-Path $classes $extension

    if (-not (Test-Path -Path $extKey)) {
        continue
    }

    $properties = Get-ItemProperty -Path $extKey -ErrorAction SilentlyContinue
    if ($null -ne $properties -and $properties.'(default)' -eq $progId) {
        Set-Item -Path $extKey -Value ''
    }

    $openWithKey = Join-Path $extKey 'OpenWithProgids'
    if (Test-Path -Path $openWithKey) {
        Remove-ItemProperty -Path $openWithKey -Name $progId -ErrorAction SilentlyContinue
    }
}

Remove-Item -Path (Join-Path $classes $progId) -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path (Join-Path $classes 'Applications\picasa-rs.exe') `
    -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\picasa-rs.exe' `
    -Recurse -Force -ErrorAction SilentlyContinue

if ($RemoveFiles) {
    Remove-Item -Path $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host "Removed $InstallDir"
}

Add-Type -Namespace PicasaRs -Name Shell -MemberDefinition @'
[DllImport("shell32.dll")]
public static extern void SHChangeNotify(int eventId, uint flags, IntPtr item1, IntPtr item2);
'@
[PicasaRs.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host "picasa-rs file associations removed."
