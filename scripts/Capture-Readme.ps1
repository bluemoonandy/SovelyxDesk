#Requires -Version 7.0
[CmdletBinding()]
param(
    [string]$Executable = '',
    [ValidateSet('auto', 'light', 'dark')][string]$PreviewTheme = 'auto',
    [ValidateSet('1', '1.25', '1.5', '2')][string]$ScaleFactor = '1',
    [string]$OutputDirectory = '',
    [ValidateSet(0,1,2,3,6)][int]$DeviceCount = 2,
    [int]$WindowWidth = 0,
    [int]$WindowHeight = 0,
    [int]$SessionPort = 0,
    [ValidateSet("en", "zh-CN")][string]$Language = "en"
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
    [StructLayout(LayoutKind.Sequential)] public struct Point { public int X, Y; }
    [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref Point point);
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
                if (name.Length > 0) { found = h; return false; }
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
elseif ($DeviceCount -ge 3) {
    $settings.profiles += @(1..($DeviceCount - 2) | ForEach-Object { @{ id = [guid]::NewGuid().ToString(); name = "Office Mac $_"; host = "mac-$_.example.test"; port = 5900; authentication = 'vnc_password'; has_saved_credentials = $false } })
}
[IO.File]::WriteAllText((Join-Path $temporary 'settings.json'), ($settings | ConvertTo-Json -Depth 8))
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = (Resolve-Path -LiteralPath $Executable).Path
$start.Arguments = "--language $Language --data-dir `"$temporary`""
if ($PreviewTheme -ne 'auto') { $start.Arguments += " --preview-theme $PreviewTheme" }
$start.EnvironmentVariables['SLINT_SCALE_FACTOR'] = $ScaleFactor
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardError = $true
$process = [Diagnostics.Process]::Start($start)
$handle = [IntPtr]::Zero

$script:UiLanguage = $Language
$script:UiCatalog = Get-Content -LiteralPath (Join-Path $root 'translations\zh_CN.json') -Encoding UTF8 -Raw | ConvertFrom-Json -AsHashtable
function Ui-Text([string]$Text) {
    if ($script:UiLanguage -eq 'en') { return $Text }
    if ($script:UiCatalog.Contains($Text)) { return [string]$script:UiCatalog[$Text] }
    foreach ($prefix in @('More actions for ', 'Mac: ')) {
        if ($Text.StartsWith($prefix)) { return ([string]$script:UiCatalog[$prefix]) + $Text.Substring($prefix.Length) }
    }
    return $Text
}

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
    $suffix = "-$Language-$PreviewTheme-$ScaleFactor-$DeviceCount-${WindowWidth}x${WindowHeight}"
    Capture-OwnedWindow (Join-Path $images "connections$suffix.png")
    $window = [Windows.Automation.AutomationElement]::FromHandle($handle)
    function Find-Control([string]$Name) {
        $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text $Name))
        $deadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            $element = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
            if ($null -eq $element) { Start-Sleep -Milliseconds 50 }
        } while ($null -eq $element -and [DateTime]::UtcNow -lt $deadline)
        if ($null -eq $element) { throw "Control not found: $Name" }
        return $element
    }
    function Invoke-Control([string]$Name) {
        $nameCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text $Name))
        $typeCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button)
        $condition = [Windows.Automation.AndCondition]::new($nameCondition, $typeCondition)
        $deadline = [DateTime]::UtcNow.AddSeconds(5)
        $invoke = $null
        do {
            $button = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
            if ($null -ne $button -and $button.Current.IsEnabled -and
                $button.TryGetCurrentPattern([Windows.Automation.InvokePattern]::Pattern, [ref]$invoke)) { break }
            $invoke = $null
            Start-Sleep -Milliseconds 50
        } while ([DateTime]::UtcNow -lt $deadline)
        if ($null -eq $invoke) { throw "Invokable button not found: $Name" }
        $invoke.Invoke()
        Start-Sleep -Milliseconds 250
    }
    function Set-Control([string]$Name, [string]$Value) {
        $nameCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text $Name))
        $editCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Edit)
        $comboCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::ComboBox)
        $typeCondition = [Windows.Automation.OrCondition]::new($editCondition, $comboCondition)
        $condition = [Windows.Automation.AndCondition]::new($nameCondition, $typeCondition)
        $deadline = [DateTime]::UtcNow.AddSeconds(5)
        do {
            $control = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
            if ($null -eq $control) { Start-Sleep -Milliseconds 50 }
        } while ($null -eq $control -and [DateTime]::UtcNow -lt $deadline)
        if ($null -eq $control) { throw "Editable control not found: $Name" }
        if ($control.Current.ControlType -eq [Windows.Automation.ControlType]::ComboBox) {
            # Cupertino ComboBox does not apply UIA SetValue; use its public arrow-key interaction.
            $choices = switch ($Name) {
                'Login method' { @('Mac account', 'VNC password') }
                'Credential protection' { @('Quick Connect', 'Windows Hello', 'Manual') }
                'Language' { @('System Default', '简体中文', 'English') }
                'Display scale' { @('Fit to Window', 'Actual Size (1:1 physical pixels)', '100% of Fit', '125% of Fit', '150% of Fit', '175% of Fit', '200% of Fit') }
                default { throw "No fixture choices for combo: $Name" }
            }
            $index = [Array]::IndexOf($choices, $Value)
            $Value = if ($Name -eq 'Language' -and $Value -eq 'English') { 'English' } else { Ui-Text $Value }
            if ($index -lt 0) { throw "Unknown combo value: $Value" }
            $control.SetFocus()
            Start-Sleep -Milliseconds 150
            function Send-ComboArrow([int]$Key, [int]$Scan) {
                $down = [long]1 -bor ([long]$Scan -shl 16) -bor [long]0x01000000
                $up = $down -bor [long]3221225472
                if (-not [MacCanvasScreenshot]::PostMessage($handle, 0x100, [IntPtr]$Key, [IntPtr]$down)) { throw 'Cannot post fixture key down.' }
                if (-not [MacCanvasScreenshot]::PostMessage($handle, 0x101, [IntPtr]$Key, [IntPtr]$up)) { throw 'Cannot post fixture key up.' }
                Start-Sleep -Milliseconds 50
            }
            for ($step = 0; $step -lt $choices.Count; $step++) { Send-ComboArrow 0x26 0x48 }
            for ($step = 0; $step -lt $index; $step++) { Send-ComboArrow 0x28 0x50 }
            $deadline = [DateTime]::UtcNow.AddSeconds(5)
            do {
                $actual = $control.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value
                if ($actual -cne $Value) { Start-Sleep -Milliseconds 50 }
            } while ($actual -cne $Value -and [DateTime]::UtcNow -lt $deadline)
            if ($actual -cne $Value) { throw "Combo did not select '$Value': '$actual'" }
        } else {
            $control.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($Value)
        }
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
            [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Mac: Studio Mac mini'))
        )
        $card = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $cardCondition)
        if ($null -eq $card) { throw 'Device card was not published.' }
        Assert-InWindow $card 'Device card'
        if ($card.Current.BoundingRectangle.Width -gt $(if ($DeviceCount -eq 1) { 461 } else { 381 }) * [double]$ScaleFactor) { throw 'Device card stretched beyond its maximum width.' }
        if ($DeviceCount -ge 4) {
            $search = Find-Control 'Search saved Macs'
            if ($search.Current.BoundingRectangle.Width -gt 321 * [double]$ScaleFactor) { throw 'Search stretched beyond its maximum width.' }
            Set-Control 'Search saved Macs' 'there-is-no-such-mac'
            $null = Find-Control 'No Macs match this search.'
            Capture-OwnedWindow (Join-Path $images "no-results$suffix.png")
            Invoke-Control 'Clear search'
        }
    } else {
        $null = Find-Control 'New connection'
    }
    # New connection forms use synthetic data and never contact a real Mac.
    Invoke-Control 'New connection'
    Assert-InWindow (Find-Control 'Connect to Mac') 'Save & Connect'
    Set-Control 'Machine name' 'Test Mac'
    Set-Control 'Mac address' '127.0.0.1'
    Set-Control 'Mac password' 'fixture'
    Set-Control 'Machine name' 'Renamed test Mac'
    Set-Control 'Mac password' ''
    Capture-OwnedWindow (Join-Path $images "new-mac-account$suffix.png")
    Set-Control 'Mac password' 'fixture'
    Set-Control 'Login method' 'VNC password'
    $usernameCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Mac username'))
    if ($null -ne $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $usernameCondition)) { throw 'VNC password form exposed a Mac username.' }
    Capture-OwnedWindow (Join-Path $images "new-vnc-password$suffix.png")
    Invoke-Control 'Close connection editor'
    if ($DeviceCount -ge 4) { Set-Control 'Search saved Macs' 'language-input-preserved' }
    $searchBefore = 'language-input-preserved'
    Invoke-Control 'Settings'
    $opposite = if ($script:UiLanguage -eq 'en') { '简体中文' } else { 'English' }
    Set-Control 'Language' $opposite
    $script:UiLanguage = if ($script:UiLanguage -eq 'en') { 'zh-CN' } else { 'en' }
    Start-Sleep -Milliseconds 300
    Capture-OwnedWindow (Join-Path $images "language-switched$suffix.png")
    Set-Control 'Language' $(if ($Language -eq 'en') { 'English' } else { '简体中文' })
    $script:UiLanguage = $Language
    Start-Sleep -Milliseconds 300
    Capture-OwnedWindow (Join-Path $images "settings$suffix.png")
    Invoke-Control 'Close'
    if ($DeviceCount -ge 4) {
        if ((Find-Control 'Search saved Macs').GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -cne $searchBefore) { throw 'Language switching cleared the search input.' }
        Invoke-Control 'Clear search'
    }
    if ($DeviceCount -lt 2) {
        Write-Output 'Empty/single-device, form and compact layout checks: PASS'
        return
    }
    $buttonCondition = [Windows.Automation.AndCondition]::new(
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button),
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'More actions for Studio Mac mini'))
    )
    $details = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $buttonCondition)
    if ($null -eq $details) { throw 'Saved-machine details button was not found.' }
    Invoke-Control 'More actions for Studio Mac mini'
    Invoke-Control 'Edit Mac'
    Start-Sleep -Milliseconds 400
    $passwordCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Mac password'))
    if ($null -ne $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $passwordCondition)) {
        throw 'Saved-login details exposed a password control.'
    }
    $helloCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Windows Hello protected'))
    if ($null -eq $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $helloCondition)) {
        throw 'Legacy Windows Hello protection is not visible.'
    }
    if ($true) { Capture-OwnedWindow (Join-Path $images "saved-login$suffix.png") }
    $allCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Close connection editor'))
    $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $allCondition).
        GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
    Start-Sleep -Milliseconds 150
    if ($DeviceCount -lt 4 -and $SessionPort -eq 0) { Write-Output 'Compact device layout and saved-password non-disclosure: PASS'; return }
    $searchCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Search saved Macs'))
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    do {
        $search = $window.FindFirst([Windows.Automation.TreeScope]::Descendants, $searchCondition)
        if ($null -eq $search -or -not $search.Current.IsEnabled) { Start-Sleep -Milliseconds 50 }
    } while (($null -eq $search -or -not $search.Current.IsEnabled) -and [DateTime]::UtcNow -lt $deadline)
    if ($null -eq $search -or -not $search.Current.IsEnabled) { throw 'Saved-Mac list did not become enabled after closing the editor.' }
    $search.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('MacBook')
    $deadline = [DateTime]::UtcNow.AddSeconds(5)
    $buttonCondition = [Windows.Automation.AndCondition]::new(
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'More actions for MacBook Pro')),
        [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Button)
    )
    $groupCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::Group)
    do {
        $rows = $window.FindAll([Windows.Automation.TreeScope]::Descendants, $buttonCondition)
        # MacBook's button already exists before filtering. Wait for the old cards
        # to leave the tree so its queued Invoke cannot target a replaced row.
        $cards = @($window.FindAll([Windows.Automation.TreeScope]::Descendants, $groupCondition) | Where-Object { ([string]$_.Current.Name).StartsWith((Ui-Text 'Mac: ')) })
        $filtered = $rows.Count -eq 1 -and $cards.Count -eq 1 -and $cards[0].Current.Name -ceq (Ui-Text 'Mac: MacBook Pro')
        if (-not $filtered) { Start-Sleep -Milliseconds 50 }
    } while (-not $filtered -and [DateTime]::UtcNow -lt $deadline)
    if (-not $filtered) { throw 'Filtered device list did not publish one stable device row.' }
    $details = $rows.Item(0)
    Invoke-Control 'More actions for MacBook Pro'
    Invoke-Control 'Edit Mac'
    $addressCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Mac address'))
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
            $failureCondition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, (Ui-Text 'Connection failed'))
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
        foreach ($panel in @(@('Stream quality','quality','Close'), @('Clipboard sharing','clipboard','Done'), @('File transfer','files','Done'), @('Audio playback','audio','Done'), @('About','about','Done'))) {
            Invoke-Control $panel[0]
            Capture-OwnedWindow (Join-Path $images "$($panel[1])$suffix.png")
            if ($panel[1] -eq 'about') {
                Invoke-Control 'Open Source Licenses'
                Capture-OwnedWindow (Join-Path $images "licenses$suffix.png")
                Invoke-Control 'Close licenses'
            } else { Invoke-Control $panel[2] }
        }
        Invoke-Control 'Settings'
        Set-Control 'Language' $(if ($Language -eq 'en') { '简体中文' } else { 'English' })
        $script:UiLanguage = if ($Language -eq 'en') { 'zh-CN' } else { 'en' }
        Invoke-Control 'Close'
        Find-Control 'Disconnect' | Out-Null
        Capture-OwnedWindow (Join-Path $images "connected-language-switched$suffix.png")
        Invoke-Control 'Settings'
        Set-Control 'Language' $(if ($Language -eq 'en') { 'English' } else { '简体中文' })
        $script:UiLanguage = $Language
        Invoke-Control 'Close'

        Invoke-Control 'Display scaling'
        Set-Control 'Display scale' '150% of Fit'
        Invoke-Control 'Pan view right'
        $displayPath = Join-Path $temporary 'display-v1.json'
        $display = Get-Content -LiteralPath $displayPath -Raw | ConvertFrom-Json
        $zoom = @($display.devices | Where-Object { $_.view.mode.zoom -eq 150 })
        if ($zoom.Count -ne 1 -or $zoom[0].view.pan[0] -le 0) { throw 'Zoom and pan were not saved per device.' }
        Capture-OwnedWindow (Join-Path $images "display-zoom-menu$suffix.png")
        Invoke-Control 'Toggle local pan'
        $desktop = (Find-Control 'Remote desktop').Current.BoundingRectangle
        $point = [MacCanvasScreenshot+Point]::new()
        $point.X = [int]($desktop.Left + $desktop.Width / 2)
        $point.Y = [int]($desktop.Top + $desktop.Height / 2)
        if (-not [MacCanvasScreenshot]::ScreenToClient($handle, [ref]$point)) { throw 'Cannot map fixture pan point.' }
        $position = [long]$point.X -bor ([long]$point.Y -shl 16)
        $null = [MacCanvasScreenshot]::PostMessage($handle, 0x201, [IntPtr]1, [IntPtr]$position)
        Start-Sleep -Milliseconds 100
        $position = [long]($point.X + 30) -bor ([long]$point.Y -shl 16)
        $null = [MacCanvasScreenshot]::PostMessage($handle, 0x200, [IntPtr]1, [IntPtr]$position)
        Start-Sleep -Milliseconds 100
        $null = [MacCanvasScreenshot]::PostMessage($handle, 0x202, [IntPtr]0, [IntPtr]$position)
        Start-Sleep -Milliseconds 250
        $panned = Get-Content -LiteralPath $displayPath -Raw | ConvertFrom-Json
        if ($panned.devices[0].view.pan[0] -ge $zoom[0].view.pan[0]) { throw 'Drag did not move the local view with the pointer.' }
        Invoke-Control 'Stop local panning'
        Capture-OwnedWindow (Join-Path $images "display-zoomed$suffix.png")
        Invoke-Control 'Full screen'
        Capture-OwnedWindow (Join-Path $images "fullscreen$suffix.png")
        Invoke-Control 'Display scaling'
        if ((Find-Control 'Display scale').GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -cne (Ui-Text '150% of Fit')) { throw 'Fullscreen did not retain the display mode.' }
        Set-Control 'Display scale' 'Actual Size (1:1 physical pixels)'
        Invoke-Control 'Reset display to Fit'
        Invoke-Control 'Close display settings'
        Invoke-Control 'Exit full screen'
        $requests = @(Get-Content -LiteralPath (Join-Path $temporary 'audit\log.json') | ForEach-Object { $_ | ConvertFrom-Json } | Where-Object { $_.event_name -eq 'session.connection.requested' })
        if ($requests.Count -ne 2) { throw 'Local zoom/fullscreen unexpectedly restarted the video session.' }
        Write-Output 'Display Zoom/Actual/Fit, saved pan, fullscreen retention and no reconnect: PASS'
        Invoke-Control 'Disconnect'
        $null = Find-Control 'Search saved Macs'
        Write-Output 'Loopback failure/retry, remote desktop, fullscreen and disconnect: PASS'
    }
    Write-Output 'Synthetic screenshot capture and saved-password non-disclosure checks: PASS'
 } catch {
    if ($handle -ne [IntPtr]::Zero -and -not $process.HasExited) {
        Capture-OwnedWindow (Join-Path $images ("failure-$PreviewTheme-$ScaleFactor.png"))
        foreach ($label in @('More actions for MacBook Pro', 'Edit Mac', 'Close connection editor', 'Mac address', 'Search saved Macs')) {
            $condition = [Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty, $label)
            $matches = $window.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)
            Write-Output ("UI fixture control: {0}; count: {1}" -f $label, $matches.Count)
        }
    }
    if ($null -ne $window) {
        foreach ($element in $window.FindAll([Windows.Automation.TreeScope]::Descendants, [Windows.Automation.Condition]::TrueCondition)) {
            if ($element.Current.ControlType -eq [Windows.Automation.ControlType]::ComboBox -or $element.Current.Name -in @('Language','语言')) {
                Write-Output ("UI combo diagnostic: name={0}; type={1}; offscreen={2}; bounds={3}" -f $element.Current.Name,$element.Current.ControlType.ProgrammaticName,$element.Current.IsOffscreen,$element.Current.BoundingRectangle)
            }
        }
    }
    Write-Output $_.ScriptStackTrace
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
