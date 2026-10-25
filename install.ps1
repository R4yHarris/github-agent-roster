$ErrorActionPreference = 'Stop'

$UserHome = $env:USERPROFILE
$RosterHome = if ($env:ROSTER_MACHINE_HOME) { Join-Path $env:ROSTER_MACHINE_HOME '.roster' } else { Join-Path $UserHome '.roster' }
$InstallDir = if ($env:ROSTER_INSTALLATION_ROOT) { $env:ROSTER_INSTALLATION_ROOT } else { Join-Path $RosterHome 'installation' }
$BinDir = Join-Path $RosterHome 'bin'
$DotnetTools = Join-Path $UserHome '.dotnet\tools'
$RepoUrl = 'https://github.com/R4yHarris/github-agent-roster.git'

Write-Host '=== github-agent-roster installer (Windows PowerShell) ===' -ForegroundColor Cyan

foreach ($dir in @($RosterHome, (Join-Path $RosterHome 'vault'), $BinDir, $DotnetTools)) {
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
}
icacls $RosterHome /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" | Out-Null
icacls (Join-Path $RosterHome 'vault') /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" | Out-Null

$ScriptSourceDir = $null
if ($PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot 'Cargo.toml'))) {
  $ScriptSourceDir = $PSScriptRoot
} elseif (Test-Path 'D:\oss\github-agent-roster\Cargo.toml') {
  $ScriptSourceDir = 'D:\oss\github-agent-roster'
}

if ($ScriptSourceDir -and ((Resolve-Path $ScriptSourceDir).Path -eq $InstallDir)) {
  git -C $InstallDir submodule update --init --recursive
} elseif (Test-Path (Join-Path $InstallDir '.git')) {
  Write-Host "-> Updating installation at $InstallDir..."
  git -C $InstallDir fetch --prune origin
  git -C $InstallDir merge --ff-only origin/main
  git -C $InstallDir submodule update --init --recursive
} elseif ($ScriptSourceDir) {
  Write-Host "-> Cloning local checkout $ScriptSourceDir into $InstallDir..."
  git clone --recurse-submodules $ScriptSourceDir $InstallDir
  git -C $InstallDir remote set-url origin $RepoUrl
} else {
  Write-Host "-> Cloning $RepoUrl into $InstallDir..."
  git clone --recurse-submodules $RepoUrl $InstallDir
}

$BuiltExe = $null
if ($ScriptSourceDir -and (Test-Path (Join-Path $ScriptSourceDir 'target\release\roster.exe'))) {
  $BuiltExe = Join-Path $ScriptSourceDir 'target\release\roster.exe'
} elseif ($ScriptSourceDir -and (Test-Path (Join-Path $ScriptSourceDir 'target\debug\roster.exe'))) {
  $BuiltExe = Join-Path $ScriptSourceDir 'target\debug\roster.exe'
} else {
  Write-Host '-> Building native Rust release binary (roster.exe)...'
  cargo build --manifest-path (Join-Path $InstallDir 'Cargo.toml') --release --locked
  $BuiltExe = Join-Path $InstallDir 'target\release\roster.exe'
}

Copy-Item -Force $BuiltExe (Join-Path $BinDir 'roster.exe')
Copy-Item -Force $BuiltExe (Join-Path $DotnetTools 'roster.exe')

$SeedDir = $null
foreach ($cand in @($ScriptSourceDir, 'D:\oss\github-agent-roster')) {
  if ($cand -and (Test-Path (Join-Path $cand '.roster'))) {
    $SeedDir = Join-Path $cand '.roster'
    break
  }
}

$ConfigPath = Join-Path $RosterHome 'config.yml'
if (-not (Test-Path $ConfigPath)) {
  if ($SeedDir -and (Test-Path (Join-Path $SeedDir 'config.yml'))) {
    Copy-Item -Force (Join-Path $SeedDir 'config.yml') $ConfigPath
  } else {
    Copy-Item -Force (Join-Path $InstallDir 'roster.config.example.yml') $ConfigPath
  }
}
icacls $ConfigPath /inheritance:r /grant:r "$($env:USERNAME):F" | Out-Null

$FleetPath = Join-Path $RosterHome 'fleet.yml'
if (-not (Test-Path $FleetPath)) {
  if ($SeedDir -and (Test-Path (Join-Path $SeedDir 'fleet.yml'))) {
    Copy-Item -Force (Join-Path $SeedDir 'fleet.yml') $FleetPath
  } else {
    Set-Content -Path $FleetPath -Value "profiles: []`n" -NoNewline
  }
}
icacls $FleetPath /inheritance:r /grant:r "$($env:USERNAME):F" | Out-Null

$CapPath = Join-Path $RosterHome 'capabilities.yml'
if (-not (Test-Path $CapPath)) {
  if ($SeedDir -and (Test-Path (Join-Path $SeedDir 'capabilities.yml'))) {
    Copy-Item -Force (Join-Path $SeedDir 'capabilities.yml') $CapPath
  } else {
    Set-Content -Path $CapPath -Value "capabilities: []`n" -NoNewline
  }
}
icacls $CapPath /inheritance:r /grant:r "$($env:USERNAME):F" | Out-Null

$GitPath = (Get-Command git -ErrorAction SilentlyContinue).Source
$Node20Path = $env:ROSTER_NODE20
if (-not $Node20Path) {
  $NvmNode20 = Get-ChildItem "$env:LOCALAPPDATA\nvm\v20.*\node.exe" -ErrorAction SilentlyContinue | Select-Object -Last 1
  if ($NvmNode20) { $Node20Path = $NvmNode20.FullName }
}

$env:ROSTER_MACHINE_HOME = $UserHome
$env:ROSTER_INSTALLATION_ROOT = $InstallDir
if ($GitPath) { $env:ROSTER_GIT = $GitPath }
if ($Node20Path) { $env:ROSTER_NODE20 = $Node20Path }

[Environment]::SetEnvironmentVariable('ROSTER_MACHINE_HOME', $UserHome, 'User')
[Environment]::SetEnvironmentVariable('ROSTER_INSTALLATION_ROOT', $InstallDir, 'User')
if ($GitPath) { [Environment]::SetEnvironmentVariable('ROSTER_GIT', $GitPath, 'User') }
if ($Node20Path) { [Environment]::SetEnvironmentVariable('ROSTER_NODE20', $Node20Path, 'User') }

Write-Host ''
Write-Host "=== Machine-scoped configuration initialized in $RosterHome ===" -ForegroundColor Green
Write-Host "  Binary:        $(Join-Path $BinDir 'roster.exe')"
Write-Host "  Installation:  $InstallDir"
Write-Host "  Config:        $ConfigPath"
Write-Host "  Fleet:         $FleetPath"
Write-Host "  Capabilities:  $CapPath"
Write-Host "  Vault:         $(Join-Path $RosterHome 'vault')"
Write-Host ''
& (Join-Path $BinDir 'roster.exe') doctor
Write-Host ''
& (Join-Path $BinDir 'roster.exe') fleet list
