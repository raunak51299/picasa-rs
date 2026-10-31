#Requires -Version 5.1
<#
.SYNOPSIS
    Installs picasa-rs for the current user and registers it as an image viewer.

.DESCRIPTION
    Copies the release executable into a per-user install directory, registers a
    ProgID under HKCU\Software\Classes, and associates common image extensions
    with picasa-rs. No administrator rights are required.

.PARAMETER ExePath
    Path to picasa-rs.exe. Defaults to target\release\picasa-rs.exe relative to
    this script, or picasa-rs.exe next to the script.

.PARAMETER InstallDir
    Destination directory. Defaults to %LOCALAPPDATA%\Programs\picasa-rs.

.PARAMETER Extensions
    File extensions to associate. Defaults to the formats the viewer supports.

.EXAMPLE
    .\install.ps1

.EXAMPLE
    .\install.ps1 -ExePath .\target\release\picasa-rs.exe -InstallDir D:\Tools\picasa-rs
#>
[CmdletBinding()]
param(
    [string]$ExePath = "",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\picasa-rs'),
    [string[]]$Extensions = @(
        '.jpg', '.jpeg', '.jfif', '.png', '.webp',
        '.tif', '.tiff', '.bmp', '.gif'
    )
)

$ErrorActionPreference = 'Stop'

$progId = 'PicasaRs.Image'
$appExe = 'picasa-rs.exe'
$friendlyName = 'picasa-rs image'

function Resolve-PicasaExe {
    param([string]$Requested)

    if ($Requested) {
        if (-not (Test-Path -LiteralPath $Requested -PathType Leaf)) {
            throw "Executable not found: $Requested"
        }
        return (Resolve-Path -LiteralPath $Requested).Path
    }

    $candidates = @(
        (Join-Path $PSScriptRoot '..\..\target\release\picasa-rs.exe'),
        (Join-Path $PSScriptRoot $appExe)
    )
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }

    throw "Could not find $appExe. Build it with 'cargo build --release' or pass -ExePath."
}

function Notify-Shell {
    Add-Type -Namespace PicasaRs -Name Shell -MemberDefinition @'
[DllImport("shell32.dll")]
public static extern void SHChangeNotify(int eventId, uint flags, IntPtr item1, IntPtr item2);
'@
    [PicasaRs.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
}

$sourceExe = Resolve-PicasaExe -Requested $ExePath
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

$targetExe = Join-Path $InstallDir $appExe
Copy-Item -LiteralPath $sourceExe -Destination $targetExe -Force

$iconSource = Join-Path $PSScriptRoot 'picasa-rs.ico'
$targetIcon = Join-Path $InstallDir 'picasa-rs.ico'
if (Test-Path -LiteralPath $iconSource -PathType Leaf) {
    Copy-Item -LiteralPath $iconSource -Destination $targetIcon -Force
    $iconPath = $targetIcon
} else {
    $iconPath = $targetExe
}

$classes = 'HKCU:\Software\Classes'
$progKey = Join-Path $classes $progId

New-Item -Path $progKey -Force | Out-Null
Set-Item -Path $progKey -Value $friendlyName
New-Item -Path (Join-Path $progKey 'DefaultIcon') -Force | Out-Null
Set-Item -Path (Join-Path $progKey 'DefaultIcon') -Value "`"$iconPath`",0"
New-Item -Path (Join-Path $progKey 'shell\open\command') -Force | Out-Null
Set-Item -Path (Join-Path $progKey 'shell\open\command') -Value "`"$targetExe`" `"%1`""

foreach ($extension in $Extensions) {
    if (-not $extension.StartsWith('.')) {
        $extension = ".$extension"
    }
    $extension = $extension.ToLowerInvariant()
    $extKey = Join-Path $classes $extension

    New-Item -Path $extKey -Force | Out-Null
    Set-Item -Path $extKey -Value $progId
    New-Item -Path (Join-Path $extKey 'OpenWithProgids') -Force | Out-Null
    New-ItemProperty -Path (Join-Path $extKey 'OpenWithProgids') `
        -Name $progId -Value '' -PropertyType String -Force | Out-Null
}

$applicationKey = Join-Path $classes "Applications\$appExe"
New-Item -Path (Join-Path $applicationKey 'shell\open\command') -Force | Out-Null
Set-Item -Path (Join-Path $applicationKey 'shell\open\command') `
    -Value "`"$targetExe`" `"%1`""
New-Item -Path (Join-Path $applicationKey 'SupportedTypes') -Force | Out-Null
foreach ($extension in $Extensions) {
    if (-not $extension.StartsWith('.')) {
        $extension = ".$extension"
    }
    New-ItemProperty -Path (Join-Path $applicationKey 'SupportedTypes') `
        -Name $extension.ToLowerInvariant() -Value '' -PropertyType String -Force | Out-Null
}

$appPathKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\picasa-rs.exe'
New-Item -Path $appPathKey -Force | Out-Null
Set-Item -Path $appPathKey -Value $targetExe
New-ItemProperty -Path $appPathKey -Name 'Path' -Value $InstallDir `
    -PropertyType String -Force | Out-Null

Notify-Shell

Write-Host "picasa-rs installed to $InstallDir"
Write-Host "Associated extensions: $($Extensions -join ', ')"
Write-Host "Run '.\uninstall.ps1' to remove the associations."
