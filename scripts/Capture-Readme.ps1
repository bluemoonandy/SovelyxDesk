#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$Executable = '',
    [ValidateSet('auto', 'light', 'dark')][string]$PreviewTheme = 'auto',
    [ValidateSet('1', '1.25', '1.5', '2')][string]$ScaleFactor = '1',
    [string]$OutputDirectory = '',
    [ValidateSet(0,1,2,6)][int]$DeviceCount = 2,
    [int]$WindowWidth = 0,
    [int]$WindowHeight = 0,
    [int]$SessionPort = 0
)

$ErrorActionPreference = 'Stop'
if ($WindowWidth -eq 0) { $WindowWidth = [int](1240 * [double]$ScaleFactor) }
if ($WindowHeight -eq 0) { $WindowHeight = [int](820 * [double]$ScaleFactor) }
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class MacCanvasScreenshot {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, uint attribute, out Rect bounds, int size);
    delegate bool WindowCallback(IntPtr window, IntPtr data);
    [DllImport("user32.dll")] static extern bool EnumWindows(WindowCallback callback, IntPtr data);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder name, int maximum);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint message, IntPtr w, IntPtr l);
    public static IntPtr Find(int processId) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, data) => {
            uint owner;
            GetWindowThreadProcessId(h, out owner);
            if (owner == processId && IsWindowVisible(h)) {
                var name = new StringBuilder(128);
                GetWindowText(h, name, 128);
                if (name.ToString() == "MacCanvas") { found = h; return false; }
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
$root = Split-Path -Parent $PSScriptRoot
if (-not $Executable) { $Executable = Join-Path $root 'target\x86_64-pc-windows-msvc\debug\maccanvas.exe' }
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('maccanvas-screenshot-' + [guid]::NewGuid().ToString('N'))
$null = [IO.Directory]::CreateDirectory($temporary)
$settings = @{
    version = 1
    profiles = @(
        @{ id = [guid]::NewGuid().ToString(); name = 'Studio Mac mini'; host = '192.0.2.10'; port = 5900; authentication = 'mac_login'; has_saved_credentials = $true },
        @{ id = [guid]::NewGuid().ToString(); name = 'MacBook Pro'; host = '192.0.2.20'; port = 5900; authentication = 'mac_login'; has_saved_credentials = $false }
    )
    ctrl_as_command = $true; sidebar_open = $true; show_fps = $true
}
if ($DeviceCount -eq 0) { $settings.profiles = @() }
elseif ($DeviceCount -eq 1) { $settings.profiles = @($settings.profiles[0]) }
elseif ($DeviceCount -eq 6) {
    $settings.profiles += @(1..4 | ForEach-Object { @{ id = [guid]::NewGuid().ToString(); name = "Office Mac $_"; host = "mac-$_.example.test"; port = 5900; authentication = 'vnc_password'; has_saved_credentials = $false } })
}
[IO.File]::WriteAllText((Join-Path $temporary 'settings.json'), ($settings | ConvertTo-Json -Depth 8))
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = (Resolve-Path -LiteralPath $Executable).Path
$start.Arguments = "--data-dir `"$temporary`""
if ($PreviewTheme -ne 'auto') { $start.Arguments += " --preview-theme $PreviewTheme" }
$start.EnvironmentVariables['SLINT_SCALE_FACTOR'] = $ScaleFactor
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($start)
$handle = [IntPtr]::Zero

function Capture-OwnedWindow([string]$Destination) {
    $rectangle = [MacCanvasScreenshot+Rect]::new()
    if (-not [MacCanvasScreenshot]::GetWindowRect($handle, [ref]$rectangle)) { throw 'Cannot get the fixture window bounds.' }
    $bitmap = [Drawing.Bitmap]::new($rectangle.Right - $rectangle.Left, $rectangle.Bottom - $rectangle.Top)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $dc = $graphics.GetHdc()
    try {
        if (-not [MacCanvasScreenshot]::PrintWindow($handle, $dc, 2)) { throw 'Cannot capture the owned fixture window.' }
    } finally { $graphics.ReleaseHdc($dc) }
    try {
        $visible = [MacCanvasScreenshot+Rect]::new()
        if ([MacCanvasScreenshot]::DwmGetWindowAttribute($handle, 9, [ref]$visible, 16) -ne 0) { throw 'Cannot get visible fixture frame bounds.' }
        $crop = [Drawing.Rectangle]::new(
            [Math]::Max(0, $visible.Left - $rectangle.Left), [Math]::Max(0, $visible.Top - $rectangle.Top),
            [Math]::Min($bitmap.Width, $visible.Right - $visible.Left), [Math]::Min($bitmap.Height, $visible.Bottom - $visible.Top)
        )
        $cropped = $bitmap.Clone($crop, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
        try { $cropped.Save($Destination, [Drawing.Imaging.ImageFormat]::Png) }
        finally { $cropped.Dispose() }
    } finally { $graphics.Dispose(); $bitmap.Dispose() }
}

try {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    while ($handle -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline) {
        if ($process.HasExited) { throw $process.StandardError.ReadToEnd() }
        Start-Sleep -Milliseconds 100
        $handle = [MacCanvasScreenshot]::Find($process.Id)
    }
    if ($handle -eq [IntPtr]::Zero) { throw 'Fixture did not create its window.' }
    if (-not [MacCanvasScreenshot]::SetWindowPos($handle, [IntPtr]::Zero, 0, 0, $WindowWidth, $WindowHeight, 0x0044)) { throw 'Cannot resize fixture window.' }
    Start-Sleep -Seconds 1
    $images = if ($OutputDirectory) { [IO.Path]::GetFullPath($OutputDirectory) } else { Join-Path $root 'docs\images' }
    $null = [IO.Directory]::CreateDirectory($images)
    $suffix = "-$PreviewTheme-$ScaleFactor-$DeviceCount-${WindowWidth}x${WindowHeight}"
    Capture-OwnedWindow (Join-Path $images "connections$suffix.png")
    $window = [Windows.Automation.AutomationElement]::FromHandle($handle)
    function Find-Control([string]$Name) {
        $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $Name)
        $deadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            $element = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
            if ($null -eq $element) { Start-Sleep -Milliseconds 50 }
        } while ($null -eq $element -and [DateTime]::UtcNow -lt $deadline)
        if ($null -eq $element) { throw "Control not found: $Name" }
        return $element
    }
    function Invoke-Control([string]$Name) {
        (Find-Control $Name).GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
        Start-Sleep -Milliseconds 250
    }
    function Set-Control([string]$Name, [string]$Value) {
        (Find-Control $Name).GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($Value)
        Start-Sleep -Milliseconds 150
    }
    function Assert-InWindow($Element, [string]$Description) {
        $bounds = $Element.Current.BoundingRectangle
        $outer = $window.Current.BoundingRectangle
        if ($bounds.Width -le 0 -or $bounds.Height -le 0 -or $bounds.Left -lt $outer.Left -or $bounds.Right -gt $outer.Right + 1 -or $bounds.Bottom -gt $outer.Bottom + 1) { throw "Control outside window: $Description ($bounds)" }
    }
    if ($DeviceCount -gt 0) {
        $cardCondition = [Windows.Automation.AndCondition]::new(
            [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Group),
            [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Mac: Studio Mac mini')
        )
        $card = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $cardCondition)
        if ($null -eq $card) { throw 'Device card was not published.' }
        Assert-InWindow $card 'Device card'
        if ($card.Current.BoundingRectangle.Width -gt 421 * [double]$ScaleFactor) { throw 'Device card stretched beyond its maximum width.' }
        $search = Find-Control 'Search saved Macs'
        if ($search.Current.BoundingRectangle.Width -gt 321 * [double]$ScaleFactor) { throw 'Search stretched beyond its maximum width.' }
        Set-Control 'Search saved Macs' 'there-is-no-such-mac'
        $null = Find-Control 'No Macs match this search.'
        Capture-OwnedWindow (Join-Path $images "no-results$suffix.png")
        Invoke-Control 'Clear search'
    } else {
        $null = Find-Control 'New connection'
    }
    # New connection forms use synthetic data and never contact a real Mac.
    Invoke-Control 'New connection'
    Assert-InWindow (Find-Control 'Connect to Mac') 'Save & Connect'
    Set-Control 'Machine name' 'Test Mac'
    Set-Control 'Mac address' '127.0.0.1'
    Capture-OwnedWindow (Join-Path $images "new-mac-account$suffix.png")
    Set-Control 'Login method' 'VNC password'
    $usernameCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Mac username')
    if ($null -ne $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $usernameCondition)) { throw 'VNC password form exposed a Mac username.' }
    Capture-OwnedWindow (Join-Path $images "new-vnc-password$suffix.png")
    Invoke-Control 'Close connection editor'
    Invoke-Control 'Settings'
    Capture-OwnedWindow (Join-Path $images "settings$suffix.png")
    Invoke-Control 'Close'
    if ($DeviceCount -lt 2) {
        Write-Output 'Empty/single-device, form and compact layout checks: PASS'
        return
    }
    $buttonCondition = [Windows.Automation.AndCondition]::new(
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button),
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Details')
    )
    $details = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    if ($null -eq $details) { throw 'Saved-machine details button was not found.' }
    $details.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Start-Sleep -Milliseconds 400
    $passwordCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Mac password')
    if ($null -ne $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $passwordCondition)) {
        throw 'Saved-login details exposed a password control.'
    }
    $helloCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Windows Hello protected')
    if ($null -eq $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $helloCondition)) {
        throw 'Legacy Windows Hello protection is not visible.'
    }
    if ($true) { Capture-OwnedWindow (Join-Path $images "saved-login$suffix.png") }
    $allCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Close connection editor')
    $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $allCondition).
        GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Start-Sleep -Milliseconds 150
    $searchCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Search saved Macs')
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $search = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $searchCondition)
        if ($null -eq $search -or -not $search.Current.IsEnabled) { Start-Sleep -Milliseconds 50 }
    } while (($null -eq $search -or -not $search.Current.IsEnabled) -and [DateTime]::UtcNow -lt $deadline)
    if ($null -eq $search -or -not $search.Current.IsEnabled) { throw 'Saved-Mac list did not become enabled after closing the editor.' }
    $search.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('MacBook')
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $rows = $window.FindAll([Windows.Automation.TreeScope]::Descendants, $buttonCondition)
        if ($rows.Count -ne 1) { Start-Sleep -Milliseconds 50 }
    } while ($rows.Count -ne 1 -and [DateTime]::UtcNow -lt $deadline)
    if ($rows.Count -ne 1) { throw 'Filtered device list did not publish one stable device row.' }
    $details = $rows.Item(0)
    $details.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    $addressCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Mac address')
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $address = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $addressCondition)
        if ($null -eq $address) { Start-Sleep -Milliseconds 50 }
    } while ($null -eq $address -and [DateTime]::UtcNow -lt $deadline)
    if ($null -eq $address) { throw 'Saved-Mac details did not open after selecting the filtered result.' }
    if ($address.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -cne '192.0.2.20') {
        throw 'Search selected the wrong saved Mac.'
    }
    Capture-OwnedWindow (Join-Path $images "edit-mac$suffix.png")
    if ($SessionPort -gt 0) {
        Set-Control 'Mac address' '127.0.0.1'
        Set-Control 'VNC port' "$SessionPort"
        Set-Control 'Login method' 'VNC password'
        Set-Control 'Credential protection' 'Manual'
        Set-Control 'Mac password' 'fixture'
        Invoke-Control 'Connect to Mac'
        Capture-OwnedWindow (Join-Path $images "connecting$suffix.png")
        $deadline = [DateTime]::UtcNow.AddSeconds(15)
        do {
            $failureCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, 'Connection failed')
            $failure = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $failureCondition)
            if ($null -eq $failure) { Start-Sleep -Milliseconds 100 }
        } while ($null -eq $failure -and [DateTime]::UtcNow -lt $deadline)
        if ($null -eq $failure) { throw 'Expected loopback connection failure was not shown.' }
        Capture-OwnedWindow (Join-Path $images "connection-failed$suffix.png")
        Set-Control 'Mac password' 'fixture'
        Invoke-Control 'Connect to Mac'
        $null = Find-Control 'Disconnect'
        Start-Sleep -Seconds 1
        Capture-OwnedWindow (Join-Path $images "connected$suffix.png")
        Invoke-Control 'Full screen'
        Capture-OwnedWindow (Join-Path $images "fullscreen$suffix.png")
        Invoke-Control 'Exit full screen'
        Invoke-Control 'Disconnect'
        $null = Find-Control 'Search saved Macs'
        Write-Output 'Loopback failure/retry, remote desktop, fullscreen and disconnect: PASS'
    }
    Write-Output 'Synthetic screenshot capture and saved-password non-disclosure checks: PASS'
 } catch {
    if ($handle -ne [IntPtr]::Zero -and -not $process.HasExited) {
        Capture-OwnedWindow (Join-Path $images ("failure-$PreviewTheme-$ScaleFactor.png"))
        foreach ($label in @('Details', 'Close connection editor', 'Mac address', 'Search saved Macs')) {
            $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $label)
            $matches = $window.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
            Write-Output ("UI fixture control: {0}; count: {1}" -f $label, $matches.Count)
        }
    }
    Write-Output $_.ToString()
    throw
} finally {
    if (-not $process.HasExited) {
        $null = [MacCanvasScreenshot]::PostMessage($handle, 0x10, [IntPtr]::Zero, [IntPtr]::Zero)
        if (-not $process.WaitForExit(5000)) { Stop-Process -Id $process.Id -Force }
    }
    $stderr = $process.StandardError.ReadToEnd()
    if ($stderr) { Write-Output $stderr }
    [IO.File]::Delete((Join-Path $temporary 'settings.json'))
    if (@(Get-ChildItem -LiteralPath $temporary -Force).Count -eq 0) { [IO.Directory]::Delete($temporary, $false) }
    if ($process.ExitCode -ne 0) { throw "Screenshot fixture exited with $($process.ExitCode)." }
}
