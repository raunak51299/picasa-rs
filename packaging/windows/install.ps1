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
$running = Get-Process -Name 'picasa-rs' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $targetExe }
if ($running) {
    throw "picasa-rs is running from $targetExe. Close it and run the installer again."
}
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
$openCommand = "`"$targetExe`" `"%1`""

New-Item -Path $progKey -Force | Out-Null
Set-Item -Path $progKey -Value $friendlyName
New-ItemProperty -Path $progKey -Name 'FriendlyTypeName' -Value $friendlyName `
    -PropertyType String -Force | Out-Null
New-Item -Path (Join-Path $progKey 'DefaultIcon') -Force | Out-Null
Set-Item -Path (Join-Path $progKey 'DefaultIcon') -Value "`"$iconPath`",0"
New-Item -Path (Join-Path $progKey 'shell\open') -Force | Out-Null
Set-ItemProperty -Path (Join-Path $progKey 'shell\open') -Name 'MUIVerb' `
    -Value 'Open with picasa-rs' -Type String -Force
New-Item -Path (Join-Path $progKey 'shell\open\command') -Force | Out-Null
Set-Item -Path (Join-Path $progKey 'shell\open\command') -Value $openCommand

foreach ($extension in $Extensions) {
    if (-not $extension.StartsWith('.')) {
        $extension = ".$extension"
    }
    $extension = $extension.ToLowerInvariant()
    $extKey = Join-Path $classes $extension

    New-Item -Path $extKey -Force | Out-Null
    # Only claim the class default when nothing else owns it; overwriting an
    # existing default would change the file's perceived type and thumbnails.
    $existingDefault = (Get-Item -Path $extKey).GetValue('')
    if ([string]::IsNullOrEmpty($existingDefault)) {
        Set-Item -Path $extKey -Value $progId
    }
    New-Item -Path (Join-Path $extKey 'OpenWithProgids') -Force | Out-Null
    New-ItemProperty -Path (Join-Path $extKey 'OpenWithProgids') `
        -Name $progId -Value '' -PropertyType String -Force | Out-Null
}

$applicationKey = Join-Path $classes "Applications\$appExe"
New-Item -Path $applicationKey -Force | Out-Null
New-ItemProperty -Path $applicationKey -Name 'FriendlyAppName' -Value 'picasa-rs' `
    -PropertyType String -Force | Out-Null
New-ItemProperty -Path $applicationKey -Name 'ApplicationName' -Value 'picasa-rs' `
    -PropertyType String -Force | Out-Null
New-ItemProperty -Path $applicationKey -Name 'ApplicationIcon' -Value "`"$iconPath`",0" `
    -PropertyType String -Force | Out-Null
New-Item -Path (Join-Path $applicationKey 'shell\open') -Force | Out-Null
Set-ItemProperty -Path (Join-Path $applicationKey 'shell\open') -Name 'MUIVerb' `
    -Value 'Open with picasa-rs' -Type String -Force
New-Item -Path (Join-Path $applicationKey 'shell\open\command') -Force | Out-Null
Set-Item -Path (Join-Path $applicationKey 'shell\open\command') -Value $openCommand
New-Item -Path (Join-Path $applicationKey 'SupportedTypes') -Force | Out-Null
foreach ($extension in $Extensions) {
    if (-not $extension.StartsWith('.')) {
        $extension = ".$extension"
    }
    New-ItemProperty -Path (Join-Path $applicationKey 'SupportedTypes') `
        -Name $extension.ToLowerInvariant() -Value '' -PropertyType String -Force | Out-Null
}

# Register with Default Programs so picasa-rs appears in Windows' "Open with"
# and "Default apps" pickers even for extensions whose default is another app.
$capabilitiesKey = 'HKCU:\Software\picasa-rs\Capabilities'
New-Item -Path $capabilitiesKey -Force | Out-Null
New-ItemProperty -Path $capabilitiesKey -Name 'ApplicationName' -Value 'picasa-rs' `
    -PropertyType String -Force | Out-Null
New-ItemProperty -Path $capabilitiesKey -Name 'ApplicationDescription' `
    -Value 'A fast, transparent photo viewer with zoom, slideshow, and fixes.' `
    -PropertyType String -Force | Out-Null
New-ItemProperty -Path $capabilitiesKey -Name 'ApplicationIcon' `
    -Value "`"$iconPath`",0" -PropertyType String -Force | Out-Null
$fileAssociationsKey = Join-Path $capabilitiesKey 'FileAssociations'
New-Item -Path $fileAssociationsKey -Force | Out-Null
foreach ($extension in $Extensions) {
    if (-not $extension.StartsWith('.')) {
        $extension = ".$extension"
    }
    New-ItemProperty -Path $fileAssociationsKey -Name $extension.ToLowerInvariant() `
        -Value $progId -PropertyType String -Force | Out-Null
}
New-Item -Path 'HKCU:\Software\RegisteredApplications' -Force | Out-Null
New-ItemProperty -Path 'HKCU:\Software\RegisteredApplications' -Name 'picasa-rs' `
    -Value 'Software\picasa-rs\Capabilities' -PropertyType String -Force | Out-Null

# Add a direct "Open with picasa-rs" verb to image files and folders. These
# verbs do not change the default handler; they only add a context-menu entry.
function Add-ShellVerb {
    param(
        [string]$Key,
        [string]$Command
    )

    New-Item -Path $Key -Force | Out-Null
    Set-ItemProperty -Path $Key -Name 'MUIVerb' -Value 'Open with picasa-rs' `
        -Type String -Force
    Set-ItemProperty -Path $Key -Name 'Icon' -Value "`"$iconPath`",0" `
        -Type String -Force
    Set-ItemProperty -Path $Key -Name 'MultiSelectModel' -Value 'Single' `
        -Type String -Force
    New-Item -Path (Join-Path $Key 'command') -Force | Out-Null
    Set-Item -Path (Join-Path $Key 'command') -Value $Command
}

foreach ($extension in $Extensions) {
    if (-not $extension.StartsWith('.')) {
        $extension = ".$extension"
    }
    Add-ShellVerb `
        -Key "HKCU:\Software\Classes\SystemFileAssociations\$($extension.ToLowerInvariant())\shell\PicasaRs" `
        -Command $openCommand
}Add-ShellVerb -Key 'HKCU:\Software\Classes\Directory\shell\PicasaRs' `
    -Command $openCommand
Add-ShellVerb -Key 'HKCU:\Software\Classes\Directory\Background\shell\PicasaRs' `
    -Command "`"$targetExe`" `"%V`""

$appPathKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\picasa-rs.exe'
New-Item -Path $appPathKey -Force | Out-Null
Set-Item -Path $appPathKey -Value $targetExe
New-ItemProperty -Path $appPathKey -Name 'Path' -Value $InstallDir `
    -PropertyType String -Force | Out-Null

Notify-Shell

Write-Host "picasa-rs installed to $InstallDir"
Write-Host "Associated extensions: $($Extensions -join ', ')"
Write-Host "Run '.\uninstall.ps1' to remove the associations."
