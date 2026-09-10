param([Parameter(Mandatory)][ValidateSet('x64','arm64')][string]$Architecture)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Deliberately restricted to disposable GitHub-hosted Windows machines. Never
# execute installer acceptance on a developer's active proxy computer.
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or
    $env:GITHUB_REPOSITORY -ne 'CMMUU/serylane' -or $env:RUNNER_OS -ne 'Windows') {
    throw 'Installer acceptance is restricted to disposable Serylane GitHub-hosted Windows runners.'
}
$repo = Split-Path $PSScriptRoot -Parent
$version = (Get-Content -LiteralPath (Join-Path $repo 'package.json') -Raw | ConvertFrom-Json).version
$testRoot = Join-Path $env:RUNNER_TEMP ('serylane-install-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$dataRoot = Join-Path $env:APPDATA 'com.cmmuu.mihomodesktop'
if (Test-Path -LiteralPath $dataRoot) { throw 'Runner contains existing application data; refusing to touch it.' }
foreach ($brand in @('RouteDeck','Serylane')) {
    if (Test-Path -LiteralPath "HKCU:\Software\cmmuu\$brand") {
        throw 'Runner contains existing product installation records; refusing to touch them.'
    }
}
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$proxyKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
function Read-Run([string]$name) {
    if (!(Test-Path -LiteralPath $runKey)) { return $null }
    # The registry provider throws a terminating exception for a missing value,
    # even with SilentlyContinue. Missing startup entries are the expected case.
    (Get-Item -LiteralPath $runKey).GetValue($name, $null)
}
if ((Read-Run 'Serylane') -or (Read-Run 'RouteDeck')) { throw 'Runner contains an existing login registration.' }
function Proxy-Snapshot {
    $value = Get-ItemProperty -LiteralPath $proxyKey
    $fields = [ordered]@{}
    foreach ($name in @('ProxyEnable','ProxyServer','ProxyOverride','AutoConfigURL')) {
        $fields[$name] = if ($value.PSObject.Properties[$name]) { $value.$name } else { $null }
    }
    $fields | ConvertTo-Json -Compress
}
$proxyBefore = Proxy-Snapshot
# Process.MainWindowHandle may select Tauri's tray/event-loop helper window.
# Inspect the actual named application window, including hidden top-level windows.
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class InstallerWindows {
    public delegate bool Callback(IntPtr handle, IntPtr parameter);
    [DllImport("user32.dll")] static extern bool EnumWindows(Callback callback, IntPtr parameter);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr handle, out uint process);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr handle, StringBuilder text, int maximum);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr handle);
    public sealed class Window { public string Title; public bool Visible; }
    public static Window[] ForProcess(int process) {
        var found = new List<Window>();
        EnumWindows((handle, parameter) => {
            uint owner; GetWindowThreadProcessId(handle, out owner);
            if (owner == process) {
                var title = new StringBuilder(512);
                GetWindowText(handle, title, title.Capacity);
                found.Add(new Window { Title=title.ToString(), Visible=IsWindowVisible(handle) });
            }
            return true;
        }, IntPtr.Zero);
        return found.ToArray();
    }
}
'@
function Run-Installer([string]$file, [string]$arguments) {
    $process = Start-Process -FilePath $file -ArgumentList $arguments -PassThru -WindowStyle Hidden
    if (!$process.WaitForExit(180000)) { throw 'Installer did not finish within three minutes.' }
    if ($process.ExitCode -notin @(0,3010)) { throw "Installer failed with exit code $($process.ExitCode)." }
}
function Install-Package([string]$file, [string]$directory) {
    if ($file.EndsWith('.msi')) {
        Run-Installer 'msiexec.exe' "/i `"$file`" /qn /norestart INSTALLDIR=`"$directory`""
    } else {
        Run-Installer $file "/S /D=$directory"
    }
}
function Uninstall-Package([string]$file, [string]$directory) {
    $resolved = [IO.Path]::GetFullPath($directory)
    if (!$resolved.StartsWith([IO.Path]::GetFullPath($testRoot) + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Uninstall target escaped the disposable test directory.'
    }
    if ($file.EndsWith('.msi')) {
        Run-Installer 'msiexec.exe' "/x `"$file`" /qn /norestart"
    } else {
        Run-Installer (Join-Path $resolved 'uninstall.exe') '/S'
    }
    # NSIS can hand off to its temporary uninstaller process before exiting.
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ((Test-Path -LiteralPath (Join-Path $resolved 'serylane.exe')) -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 500
    }
    if (Test-Path -LiteralPath (Join-Path $resolved 'serylane.exe')) { throw 'Uninstall left the main executable behind.' }
    Start-Sleep -Seconds 1
}
function Write-Fixture([bool]$login) {
    New-Item -ItemType Directory -Path $dataRoot -Force | Out-Null
    @{
        schemaVersion=1; locale='zh-CN'; theme='system'; launchAtLogin=$login;
        silentStartup=$true; restoreLastSession=$true; showGlobalTraffic=$false;
        networkMode='system_proxy'; mixedPort=17890; controllerPort=19090;
        controllerSecret='isolated-installer-fixture'; updateChannel='stable';
        autoCheckUpdates=$false; autoDownloadUpdates=$false; updateSource='auto';
        diagnosticsRetentionDays=7; appLogRetentionDays=3
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $dataRoot 'settings.json') -Encoding utf8
    @{
        schemaVersion=1; activeProfileId=$null; activeRevisionId=$null;
        systemProxySnapshotPresent=$false; cleanShutdown=$true; desiredRunning=$false; updatedAt=$null
    } | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $dataRoot 'state.json') -Encoding utf8
    'preserve-upgrade-data' | Set-Content -LiteralPath (Join-Path $dataRoot 'installer-sentinel.txt') -Encoding utf8
}
function Reset-FixtureInstallLocations {
    # NSIS intentionally preserves installation-location metadata on uninstall.
    # It must not redirect the next independent MSI scenario to the preceding fixture.
    foreach ($brand in @('RouteDeck','Serylane')) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\cmmuu\$brand", $true)
        if (!$key) { continue }
        try {
            foreach ($name in @('', 'InstallDir')) {
                $value = $key.GetValue($name, $null)
                if (!$value) { continue }
                $resolved = [IO.Path]::GetFullPath([string]$value)
                if (!$resolved.StartsWith([IO.Path]::GetFullPath($testRoot) + '\', [StringComparison]::OrdinalIgnoreCase)) {
                    throw 'Installer location is not owned by this isolated test.'
                }
                $key.DeleteValue($name, $false)
            }
        } finally { $key.Dispose() }
    }
}
function Assert-Application([string]$executable, [bool]$login) {
    $settings = Get-Content -LiteralPath (Join-Path $dataRoot 'settings.json') -Raw | ConvertFrom-Json
    if ($settings.networkMode -ne 'system_proxy' -or !$settings.silentStartup -or $settings.appLogRetentionDays -ne 3) {
        throw 'Installer failed to preserve the saved settings.'
    }
    foreach ($quiet in @($true, $false)) {
        $launchArgs = @{FilePath=$executable; PassThru=$true; WindowStyle='Hidden'}
        if ($quiet) { $launchArgs.ArgumentList = '--autostart' }
        $process = Start-Process @launchArgs
        try {
            $deadline = [DateTime]::UtcNow.AddSeconds(30)
            $journal = Join-Path $dataRoot 'app-log-v1.json'
            do {
                Start-Sleep -Milliseconds 500
                $process.Refresh()
                if ($process.HasExited) { throw 'Installed application exited during startup.' }
                $ready = (Test-Path -LiteralPath $journal) -and
                    ((Get-Item -LiteralPath $journal).LastWriteTimeUtc -ge $process.StartTime.ToUniversalTime())
            } until ($ready -or [DateTime]::UtcNow -gt $deadline)
            if (!$ready) { throw 'Installed application did not reach initialized logging.' }
            Start-Sleep -Seconds 3
            $process.Refresh()
            $windows = @([InstallerWindows]::ForProcess($process.Id))
            Write-Host "Window check (quiet=$quiet): $($windows | ConvertTo-Json -Compress)"
            $main = @($windows | Where-Object { $_.Title -eq 'Serylane' })
            if ($main.Count -ne 1) { throw 'Installed application must have exactly one Serylane main window.' }
            if ($quiet -and $main[0].Visible) { throw 'Silent login unexpectedly showed the Serylane main window.' }
            if (!$quiet -and !$main[0].Visible) { throw 'Manual launch failed to show the Serylane main window.' }
            if ($login) {
                $entry = Read-Run 'Serylane'
                if (!$entry -or !$entry.Contains($executable) -or !$entry.Contains('--autostart') -or (Read-Run 'RouteDeck')) {
                    throw 'Owned legacy login registration was not migrated correctly.'
                }
            } elseif ((Read-Run 'Serylane') -or (Read-Run 'RouteDeck')) { throw 'Startup was enabled without consent.' }
            if ((Proxy-Snapshot) -ne $proxyBefore) { throw 'Stopped-state login changed Windows proxy settings.' }
        } finally {
            # Only this exact process, created by this fixture, is terminated.
            if (!$process.HasExited) { Stop-Process -Id $process.Id -Force; $process.WaitForExit() }
        }
    }
}
function Assert-Shortcuts([string]$executable) {
    $shell = New-Object -ComObject WScript.Shell
    $folders = @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('CommonPrograms'),
                 [Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('CommonDesktopDirectory'))
    $found = @()
    foreach ($folder in $folders) {
        if (!$folder -or !(Test-Path -LiteralPath $folder)) { continue }
        foreach ($link in (Get-ChildItem -LiteralPath $folder -Filter '*.lnk' -Recurse)) {
            if ($link.BaseName -notin @('Serylane','RouteDeck')) { continue }
            $target = $shell.CreateShortcut($link.FullName).TargetPath
            if ($target -eq $executable) {
                if ($link.BaseName -ne 'Serylane') { throw 'Owned shortcut retained the legacy display name.' }
                $found += $link.FullName
            }
        }
    }
    if (!$found.Count) { throw 'No Serylane shortcut points to the installed application.' }
}
$base = 'https://github.com/CMMUU/serylane/releases/download/v0.7.6'
$hashes = (Invoke-WebRequest -Uri "$base/SHA256SUMS.txt").Content
if ($hashes -is [byte[]]) { $hashes = [Text.Encoding]::UTF8.GetString($hashes) }
foreach ($kind in @('nsis','msi')) {
    $suffix = if ($kind -eq 'nsis') { "$Architecture-setup.exe" } else { "${Architecture}_en-US.msi" }
    $legacyName = "RouteDeck_0.7.6_$suffix"
    $legacy = Join-Path $testRoot $legacyName
    Invoke-WebRequest -Uri "$base/$legacyName" -OutFile $legacy
    $match = [regex]::Match($hashes, '(?m)^([a-fA-F0-9]{64})\s+\*?' + [regex]::Escape($legacyName) + '\s*$')
    if (!$match.Success -or (Get-FileHash -LiteralPath $legacy -Algorithm SHA256).Hash -ne $match.Groups[1].Value) {
        throw 'Published baseline installer checksum mismatch.'
    }
    $current = Join-Path $repo "src-tauri/target/release/bundle/$kind/Serylane_${version}_$suffix"
    if (!(Test-Path -LiteralPath $current)) { throw 'Expected signed build installer is missing.' }
    foreach ($scenario in @('fresh','upgrade','upgrade-login')) {
        Reset-FixtureInstallLocations
        $directory = Join-Path $testRoot "$kind-$scenario"
        $login = $scenario -eq 'upgrade-login'
        Write-Fixture $login
        $settingsBefore = (Get-FileHash -LiteralPath (Join-Path $dataRoot 'settings.json')).Hash
        if ($scenario -ne 'fresh') {
            Install-Package $legacy $directory
            if (!(Test-Path -LiteralPath (Join-Path $directory 'routedeck.exe'))) { throw 'Baseline installation not found at requested directory.' }
            if ($login) {
                if (!(Test-Path -LiteralPath $runKey)) { New-Item -Path $runKey | Out-Null }
                New-ItemProperty -LiteralPath $runKey -Name 'RouteDeck' -Value "`"$directory\routedeck.exe`"" -PropertyType String -Force | Out-Null
            }
        }
        Install-Package $current $directory
        $executable = Join-Path $directory 'serylane.exe'
        $info = (Get-Item -LiteralPath $executable).VersionInfo
        if ($info.ProductName -ne 'Serylane' -or !$info.ProductVersion.StartsWith($version)) { throw 'Installed binary branding/version mismatch.' }
        if (Test-Path -LiteralPath (Join-Path $directory 'routedeck.exe')) { throw 'Upgrade left the old main executable behind.' }
        if ((Get-FileHash -LiteralPath (Join-Path $dataRoot 'settings.json')).Hash -ne $settingsBefore) { throw 'Upgrade rewrote application settings.' }
        Assert-Shortcuts $executable
        Assert-Application $executable $login
        Uninstall-Package $current $directory
        if ((Read-Run 'Serylane') -or (Read-Run 'RouteDeck')) { throw 'Uninstall left owned login registration.' }
        if (!(Test-Path -LiteralPath (Join-Path $dataRoot 'installer-sentinel.txt'))) { throw 'Default uninstall removed user data.' }
        Write-Host "PASS: $Architecture $kind $scenario; branding, settings, shortcuts, silent/manual startup, stopped proxy, uninstall."
    }
}
