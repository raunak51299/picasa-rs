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

    $properties = Get-Item -Path $extKey -ErrorAction SilentlyContinue
    if ($null -ne $properties -and $properties.GetValue('') -eq $progId) {
        # Remove-ItemProperty cannot delete the unnamed default value.
        $writable = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey(
            "Software\Classes\$extension", $true)
        $writable.DeleteValue('', $false)
        $writable.Close()
    }

    $openWithKey = Join-Path $extKey 'OpenWithProgids'
    if (Test-Path -Path $openWithKey) {
        Remove-ItemProperty -Path $openWithKey -Name $progId -ErrorAction SilentlyContinue
        $openWith = Get-Item -Path $openWithKey
        if ($openWith.ValueCount -eq 0 -and $openWith.SubKeyCount -eq 0) {
            Remove-Item -Path $openWithKey -Force -ErrorAction SilentlyContinue
        }
    }

    Remove-Item -Path (Join-Path "HKCU:\Software\Classes\SystemFileAssociations\$extension\shell" 'PicasaRs') `
        -Recurse -Force -ErrorAction SilentlyContinue

    # Drop the extension key if nothing else is left in it.
    $remaining = Get-Item -Path $extKey -ErrorAction SilentlyContinue
    if ($null -ne $remaining -and $remaining.ValueCount -eq 0 -and $remaining.SubKeyCount -eq 0) {
        Remove-Item -Path $extKey -Force -ErrorAction SilentlyContinue
    }
}

Remove-Item -Path (Join-Path $classes $progId) -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path (Join-Path $classes 'Applications\picasa-rs.exe') `
    -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path 'HKCU:\Software\Classes\SystemFileAssociations\image\shell\PicasaRs' `
    -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path 'HKCU:\Software\Classes\Directory\shell\PicasaRs' `
    -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path 'HKCU:\Software\Classes\Directory\Background\shell\PicasaRs' `
    -Recurse -Force -ErrorAction SilentlyContinue
Remove-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'picasa-rs' `
    -ErrorAction SilentlyContinue
Remove-Item -Path 'HKCU:\Software\picasa-rs' -Recurse -Force -ErrorAction SilentlyContinue
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
