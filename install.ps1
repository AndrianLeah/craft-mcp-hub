# Installs craft-mcp-hub (crafthub and crafthub-panel) on Windows.
#
#   irm https://raw.githubusercontent.com/AndrianLeah/craft-mcp-hub/main/install.ps1 | iex
#
# The programs go to %USERPROFILE%\.crafthub\bin (set CRAFTHUB_INSTALL_DIR for another folder),
# which is added to your PATH, and the control panel gets a Start menu shortcut.
$ErrorActionPreference = 'Stop'

$repo = 'AndrianLeah/craft-mcp-hub'
$dir = if ($env:CRAFTHUB_INSTALL_DIR) { $env:CRAFTHUB_INSTALL_DIR } else { Join-Path $env:USERPROFILE '.crafthub\bin' }
$asset = 'crafthub-windows-x86_64.zip'
$url = "https://github.com/$repo/releases/latest/download/$asset"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ('crafthub-' + [guid]::NewGuid())

New-Item -ItemType Directory -Force $tmp, $dir | Out-Null
try {
  Write-Host "Downloading $asset..."
  Invoke-WebRequest $url -OutFile (Join-Path $tmp $asset) -UseBasicParsing
  Expand-Archive (Join-Path $tmp $asset) -DestinationPath $tmp -Force
  foreach ($name in 'crafthub.exe', 'crafthub-panel.exe') {
    $source = Join-Path $tmp $name
    if (-not (Test-Path $source)) { continue }
    $target = Join-Path $dir $name
    if (Test-Path $target) {
      # A running copy can't be overwritten, but it can be moved out of the way.
      $old = "$target.old"
      if (Test-Path $old) { try { Remove-Item $old -Force } catch { $old = "$target.$(Get-Random).old" } }
      Move-Item $target $old -Force
    }
    Copy-Item $source $target
  }
} finally {
  Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}
Get-ChildItem $dir -Filter '*.old' | ForEach-Object { try { Remove-Item $_.FullName -Force -ErrorAction Stop } catch {} }

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dir) {
  [Environment]::SetEnvironmentVariable('Path', (@($userPath, $dir) | Where-Object { $_ }) -join ';', 'User')
  $env:Path = "$env:Path;$dir"
}

$panel = Join-Path $dir 'crafthub-panel.exe'
if (Test-Path $panel) {
  $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path ([Environment]::GetFolderPath('Programs')) 'Craft MCP Hub.lnk'))
  $shortcut.TargetPath = $panel
  $shortcut.WorkingDirectory = $dir
  $shortcut.Description = 'Turn the MCP servers of the Artcraft apps on and off'
  $shortcut.Save()
}

Write-Host "Installed in ${dir}: $(& (Join-Path $dir 'crafthub.exe') --version)"
Write-Host ''
Write-Host 'Next:  crafthub setup    (adds and turns on every installed Artcraft app)'
Write-Host '       crafthub panel    (opens the control panel; also in the Start menu as "Craft MCP Hub")'
