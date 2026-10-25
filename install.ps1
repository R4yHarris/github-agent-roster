$ErrorActionPreference = 'Stop'

$UserHome = $env:USERPROFILE
$RosterHome = if ($env:ROSTER_MACHINE_HOME) { Join-Path $env:ROSTER_MACHINE_HOME '.roster' } else { Join-Path $UserHome '.roster' }
$InstallDir = if ($env:ROSTER_INSTALLATION_ROOT) { $env:ROSTER_INSTALLATION_ROOT } else { Join-Path $RosterHome 'installation' }
$BinDir = Join-Path $RosterHome 'bin'
$DotnetTools = Join-Path $UserHome '.dotnet\tools'
$RepoUrl = 'https://github.com/R4yHarris/github-agent-roster.git'

Write-Host '=== github-agent-roster installer (Windows PowerShell) ===' -ForegroundColor Cyan

$CargoBin = Join-Path $UserHome '.cargo\bin'
if (Test-Path (Join-Path $CargoBin 'cargo.exe')) {
  $env:PATH = "$CargoBin;$env:PATH"
} else {
  $IsolatedToolchain = Get-ChildItem (Join-Path $UserHome '.copilot\session-state\*\files\rust-toolchain') -Directory -ErrorAction SilentlyContinue | Select-Object -Last 1
  if ($IsolatedToolchain -and (Test-Path (Join-Path $IsolatedToolchain.FullName 'cargo\bin\cargo.exe'))) {
    $root = $IsolatedToolchain.FullName
    $env:CARGO_HOME = Join-Path $root 'cargo'
    $env:RUSTUP_HOME = Join-Path $root 'rustup'
    $env:PATH = "$(Join-Path $root 'cargo\bin');$env:PATH"
    $dlltool = Join-Path $root 'llvm-dlltool-20261006\llvm-dlltool.exe'
    if (Test-Path $dlltool) {
      $env:RUSTFLAGS = "-C dlltool=$dlltool"
    }
  } elseif (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host '-> Installing Rust toolchain via rustup-init.exe...'
    $RustupInit = Join-Path $env:TEMP 'rustup-init.exe'
    Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $RustupInit -UseBasicParsing
    & $RustupInit -y
    $env:PATH = "$CargoBin;$env:PATH"
  }
}

New-Item -ItemType Directory -Force -Path $RosterHome | Out-Null
icacls $RosterHome /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" | Out-Null
foreach ($dir in @((Join-Path $RosterHome 'vault'), $BinDir, $DotnetTools)) {
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
}
icacls (Join-Path $RosterHome 'vault') /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" | Out-Null
icacls $BinDir /inheritance:r /grant:r "$($env:USERNAME):(OI)(CI)F" | Out-Null

$ScriptSourceDir = if ($env:ROSTER_INSTALL_SOURCE) { $env:ROSTER_INSTALL_SOURCE } else { $null }
if (-not $ScriptSourceDir -and $PSScriptRoot -and (Test-Path (Join-Path $PSScriptRoot 'Cargo.toml'))) {
  $ScriptSourceDir = $PSScriptRoot
}

if ($ScriptSourceDir -and ((Resolve-Path $ScriptSourceDir).Path -eq $InstallDir)) {
  git -c core.autocrlf=false -C $InstallDir submodule update --init --recursive
} elseif (Test-Path (Join-Path $InstallDir '.git')) {
  Write-Host "-> Updating installation at $InstallDir..."
  git -c core.autocrlf=false -C $InstallDir fetch --prune origin
  git -c core.autocrlf=false -C $InstallDir checkout -B main origin/main
  git -c core.autocrlf=false -C $InstallDir submodule update --init --recursive
} elseif ($ScriptSourceDir) {
  Write-Host "-> Cloning local checkout $ScriptSourceDir into $InstallDir..."
  git -c core.autocrlf=false clone --branch main --recurse-submodules $ScriptSourceDir $InstallDir
  git -C $InstallDir remote set-url origin $RepoUrl
} else {
  Write-Host "-> Cloning $RepoUrl into $InstallDir..."
  git -c core.autocrlf=false clone --branch main --recurse-submodules $RepoUrl $InstallDir
}
git -C $InstallDir config core.autocrlf false
$SubmoduleDir = Join-Path $InstallDir 'vendor\github-agent-contracts'
if (Test-Path $SubmoduleDir) {
  git -C $SubmoduleDir config core.autocrlf false
  git -C $SubmoduleDir rm --cached -r . | Out-Null
  git -C $SubmoduleDir reset --hard HEAD | Out-Null
}

$BuiltExe = $null
if ($env:ROSTER_PREBUILT_EXE -and (Test-Path $env:ROSTER_PREBUILT_EXE)) {
  $BuiltExe = $env:ROSTER_PREBUILT_EXE
} else {
  Write-Host '-> Building native Rust release binary (roster.exe)...'
  cargo build --manifest-path (Join-Path $InstallDir 'Cargo.toml') --release --locked
  $BuiltExe = Join-Path $InstallDir 'target\release\roster.exe'
}

Copy-Item -Force $BuiltExe (Join-Path $BinDir 'roster.exe')
Copy-Item -Force $BuiltExe (Join-Path $DotnetTools 'roster.exe')

$SeedDir = if ($env:ROSTER_SEED_DIR -and (Test-Path $env:ROSTER_SEED_DIR)) {
  $env:ROSTER_SEED_DIR
} elseif ($ScriptSourceDir -and (Test-Path (Join-Path $ScriptSourceDir '.roster'))) {
  Join-Path $ScriptSourceDir '.roster'
} else {
  $null
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
$CargoExe = (Get-Command cargo -ErrorAction SilentlyContinue).Source

$env:PATH = "$BinDir;$env:PATH"
$env:ROSTER_MACHINE_HOME = $UserHome
$env:ROSTER_INSTALLATION_ROOT = $InstallDir
if ($GitPath) { $env:ROSTER_GIT = $GitPath }
if ($Node20Path) { $env:ROSTER_NODE20 = $Node20Path }
if ($CargoExe) { $env:ROSTER_CARGO = $CargoExe }

$CurrentUserPath = (Get-ItemProperty -Path 'HKCU:\Environment' -Name 'Path' -ErrorAction SilentlyContinue).Path
if ($CurrentUserPath -and ($CurrentUserPath -split ';' -notcontains $BinDir)) {
  Set-ItemProperty -Path 'HKCU:\Environment' -Name 'Path' -Value "$BinDir;$CurrentUserPath"
}
Set-ItemProperty -Path 'HKCU:\Environment' -Name 'ROSTER_MACHINE_HOME' -Value $UserHome
Set-ItemProperty -Path 'HKCU:\Environment' -Name 'ROSTER_INSTALLATION_ROOT' -Value $InstallDir
if ($GitPath) { Set-ItemProperty -Path 'HKCU:\Environment' -Name 'ROSTER_GIT' -Value $GitPath }
if ($Node20Path) { Set-ItemProperty -Path 'HKCU:\Environment' -Name 'ROSTER_NODE20' -Value $Node20Path }
if ($CargoExe) { Set-ItemProperty -Path 'HKCU:\Environment' -Name 'ROSTER_CARGO' -Value $CargoExe }

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
Write-Host ''
Write-Host "Next: run 'roster onboard --discover' to configure your machine fleet." -ForegroundColor Cyan
