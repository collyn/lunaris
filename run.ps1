# Lunaris Windows Control Panel & Build Setup Script

# Setup cleanup on script exit or interruption
$bgJobs = @()
function Cleanup-Jobs {
    if ($bgJobs.Count -gt 0) {
        Write-Host "`nStopping background processes..." -ForegroundColor Yellow
        foreach ($job in $bgJobs) {
            if ($job -and $job.Id) {
                Stop-Process -Id $job.Id -Force -ErrorAction SilentlyContinue
            }
        }
    }
}

# Register cleanup handler
$currentProcess = [System.Diagnostics.Process]::GetCurrentProcess()
Register-ObjectEvent -InputObject $currentProcess -EventName Exited -Action { Cleanup-Jobs } | Out-Null

# --- Check Administrator Privileges ---
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and ($args -contains "--setup" -or $args -contains "-setup")) {
    Write-Warning "Setup requires Administrator privileges. Restarting script as Administrator..."
    Start-Process powershell -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`"", $args -Verb RunAs
    exit
}

# --- Check Prerequisites ---
function Get-MissingDeps {
    $missing = @()
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { $missing += "Git" }
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) { $missing += "CMake" }
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) { $missing += "Node.js" }
    if (-not (Get-Command python -ErrorAction SilentlyContinue)) { $missing += "Python 3" }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $missing += "Rust (Cargo)" }
    if (-not (Get-Command pkg-config -ErrorAction SilentlyContinue)) { $missing += "pkg-config" }
    
    # Check for MSVC Build Tools
    $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $hasMSVC = $false
    if (Test-Path $vsWhere) {
        $vsPath = & $vsWhere -latest -property installationPath
        if ($vsPath -and (Test-Path "$vsPath\VC\Tools\MSVC")) {
            $hasMSVC = $true
        }
    }
    if (-not $hasMSVC) { $missing += "MSVC Build Tools" }
    
    # Check for LLVM/Clang (required by bindgen)
    $llvmPath = if ($env:LIBCLANG_PATH) { $env:LIBCLANG_PATH } else { "C:\Program Files\LLVM\bin" }
    $hasLibClang = $false
    if (Test-Path "$llvmPath\libclang.dll") {
        if ((Get-Item "$llvmPath\libclang.dll").Length -gt 0) { $hasLibClang = $true }
    }
    if (Test-Path "$llvmPath\clang.dll") {
        if ((Get-Item "$llvmPath\clang.dll").Length -gt 0) { $hasLibClang = $true }
    }
    if (-not $hasLibClang) {
        $missing += "LLVM/Clang (required by bindgen)"
    }
    
    # Check vcpkg & libraries
    $vcpkgRoot = if ($env:VCPKG_ROOT) { $env:VCPKG_ROOT } else { "C:\vcpkg" }
    if (-not (Test-Path "$vcpkgRoot\vcpkg.exe")) {
        $missing += "vcpkg"
    } else {
        if (-not (Test-Path "$vcpkgRoot\installed\x64-windows\lib\avcodec.lib")) {
            $missing += "FFmpeg (via vcpkg)"
        }
        if (-not (Test-Path "$vcpkgRoot\installed\x64-windows\lib\opus.lib")) {
            $missing += "Opus (via vcpkg)"
        }
        if (-not (Test-Path "$vcpkgRoot\installed\x64-windows\lib\libssl.lib")) {
            $missing += "OpenSSL (via vcpkg)"
        }
    }
    
    # Check Qt6
    $qtRoot = if ($env:QT_ROOT_DIR) { $env:QT_ROOT_DIR } else { "C:\Qt\6.7.3\msvc2019_64" }
    if (-not (Test-Path "$qtRoot\bin\qmake.exe")) {
        $missing += "Qt 6.7.3 (msvc2019_64)"
    }
    
    return $missing
}

# --- Setup Environment ---
function Setup-Environment {
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host "          LUNARIS WINDOWS ENVIRONMENT SETUP       " -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    
    if (-not $isAdmin) {
        Write-Error "Administrator privileges are required to run setup. Please restart your shell as Administrator and run again."
        return
    }

    # 1. Install Git
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Host "Installing Git..." -ForegroundColor Yellow
        winget install --id Git.Git -e --no-upgrade --accept-source-agreements --accept-package-agreements
    }

    # 2. Install CMake
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
        Write-Host "Installing CMake..." -ForegroundColor Yellow
        winget install --id Kitware.CMake -e --no-upgrade --accept-source-agreements --accept-package-agreements
    }

    # 3. Install Python 3 (required for aqtinstall)
    if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
        Write-Host "Installing Python 3..." -ForegroundColor Yellow
        winget install --id Python.Python.3 -e --no-upgrade --accept-source-agreements --accept-package-agreements
        # Refresh path for the session
        $env:Path += ";$env:USERPROFILE\AppData\Local\Programs\Python\Python311\Scripts;$env:USERPROFILE\AppData\Local\Programs\Python\Python311"
        $env:Path += ";$env:USERPROFILE\AppData\Local\Programs\Python\Python312\Scripts;$env:USERPROFILE\AppData\Local\Programs\Python\Python312"
        $env:Path += ";$env:USERPROFILE\AppData\Local\Programs\Python\Python313\Scripts;$env:USERPROFILE\AppData\Local\Programs\Python\Python313"
    }

    # 4. Install Node.js LTS
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        Write-Host "Installing Node.js LTS..." -ForegroundColor Yellow
        winget install --id OpenJS.NodeJS.LTS -e --no-upgrade --accept-source-agreements --accept-package-agreements
    }

    # 5. Install MSVC Build Tools
    $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    $hasMSVC = $false
    if (Test-Path $vsWhere) {
        $vsPath = & $vsWhere -latest -property installationPath
        if ($vsPath -and (Test-Path "$vsPath\VC\Tools\MSVC")) {
            $hasMSVC = $true
        }
    }
    if (-not $hasMSVC) {
        Write-Host "Installing Visual Studio 2022 C++ Build Tools (MSVC)..." -ForegroundColor Yellow
        winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive" --accept-source-agreements --accept-package-agreements
    }

    # 5b. Install LLVM/Clang (required by bindgen)
    $llvmPath = "C:\Program Files\LLVM\bin"
    $needLLVM = $true
    if (Test-Path "$llvmPath\libclang.dll") {
        if ((Get-Item "$llvmPath\libclang.dll").Length -gt 0) { $needLLVM = $false }
    }
    if (Test-Path "$llvmPath\clang.dll") {
        if ((Get-Item "$llvmPath\clang.dll").Length -gt 0) { $needLLVM = $false }
    }
    if ($needLLVM) {
        Write-Host "Installing LLVM/Clang (forcing overwrite of any corrupted DLLs)..." -ForegroundColor Yellow
        winget install --id LLVM.LLVM -e --force --no-upgrade --accept-source-agreements --accept-package-agreements
    }

    # 6. Install Rustup & Toolchain
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        Write-Host "Installing Rust (Rustup)..." -ForegroundColor Yellow
        winget install --id Rustlang.Rustup -e --no-upgrade --accept-source-agreements --accept-package-agreements
        # Temporarily add cargo to session path
        $env:Path += ";$env:USERPROFILE\.cargo\bin"
    }
    Write-Host "Configuring Rust nightly toolchain..." -ForegroundColor Yellow
    & rustup toolchain install nightly-2026-04-17
    & rustup default nightly-2026-04-17

    # 7. Install pkg-config (via Chocolatey or local copy)
    if (-not (Get-Command pkg-config -ErrorAction SilentlyContinue)) {
        Write-Host "Installing pkg-config (pkgconfiglite)..." -ForegroundColor Yellow
        if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
            Write-Host "Installing Chocolatey first..." -ForegroundColor Yellow
            Set-ExecutionPolicy Bypass -Scope Process -Force
            [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.ServicePointManager]::SecurityProtocol -bor 3072
            $installScript = (New-Object System.Net.WebClient).DownloadString('https://community.chocolatey.org/install.ps1')
            Invoke-Expression $installScript
            $env:Path += ";$env:ALLUSERSPROFILE\chocolatey\bin"
        }
        & choco install pkgconfiglite -y
    }

    # 8. Install vcpkg, ffmpeg, opus, openssl
    $vcpkgRoot = "C:\vcpkg"
    if (-not (Test-Path $vcpkgRoot)) {
        Write-Host "Installing vcpkg to $vcpkgRoot..." -ForegroundColor Yellow
        & git clone https://github.com/microsoft/vcpkg.git $vcpkgRoot
    }

    # Update vcpkg to latest master branch to ensure package mirrors and ports are up to date
    Write-Host "Updating vcpkg to latest master branch (fixing MSYS2 404 mirror errors)..." -ForegroundColor Yellow
    Push-Location $vcpkgRoot
    & git fetch origin
    & git checkout -f master
    & git reset --hard origin/master
    Pop-Location

    # Re-bootstrap vcpkg to match the checked-out version
    & "$vcpkgRoot\bootstrap-vcpkg.bat"
    
    # Configure triplet to build release-only dependencies (much faster)
    Write-Host "Configuring vcpkg release-only triplet..." -ForegroundColor Yellow
    $tripletPath = "$vcpkgRoot\triplets\x64-windows.cmake"
    if (Test-Path $tripletPath) {
        $content = Get-Content $tripletPath
        if ($content -notcontains "set(VCPKG_BUILD_TYPE release)") {
            Add-Content -Path $tripletPath -Value "`nset(VCPKG_BUILD_TYPE release)"
        }
    } else {
        Set-Content -Path $tripletPath -Value "set(VCPKG_BUILD_TYPE release)"
    }

    # Remove previously installed packages to ensure we cleanly reinstall compatible versions
    Write-Host "Removing any existing vcpkg package cache..." -ForegroundColor Yellow
    & "$vcpkgRoot\vcpkg.exe" remove ffmpeg:x64-windows opus:x64-windows openssl:x64-windows --recurse 2>$null

    Write-Host "Installing FFmpeg, Opus, and OpenSSL via vcpkg (this may take a few minutes)..." -ForegroundColor Yellow
    & "$vcpkgRoot\vcpkg.exe" install ffmpeg:x64-windows opus:x64-windows openssl:x64-windows

    # 9. Install Qt6
    $qtRoot = "C:\Qt\6.7.3\msvc2019_64"
    if (-not (Test-Path $qtRoot)) {
        Write-Host "Installing aqtinstall..." -ForegroundColor Yellow
        & python -m pip install --upgrade pip
        & python -m pip install aqtinstall
        
        Write-Host "Installing Qt 6.7.3 (MSVC 64-bit) via aqtinstall..." -ForegroundColor Yellow
        & python -m aqt install-qt windows desktop 6.7.3 win64_msvc2019_64 -O C:\Qt -m qtmultimedia qtshadertools
    }

    # 10. Persist Environment Variables
    Write-Host "Persisting Environment Variables..." -ForegroundColor Yellow
    [System.Environment]::SetEnvironmentVariable("QT_ROOT_DIR", $qtRoot, [System.EnvironmentVariableTarget]::User)
    [System.Environment]::SetEnvironmentVariable("QMAKE", "$qtRoot\bin\qmake.exe", [System.EnvironmentVariableTarget]::User)
    [System.Environment]::SetEnvironmentVariable("VCPKG_ROOT", $vcpkgRoot, [System.EnvironmentVariableTarget]::User)
    [System.Environment]::SetEnvironmentVariable("PKG_CONFIG_PATH", "$vcpkgRoot\installed\x64-windows\lib\pkgconfig", [System.EnvironmentVariableTarget]::User)
    [System.Environment]::SetEnvironmentVariable("FFMPEG_DIR", "$vcpkgRoot\installed\x64-windows", [System.EnvironmentVariableTarget]::User)
    [System.Environment]::SetEnvironmentVariable("LIBCLANG_PATH", $llvmPath, [System.EnvironmentVariableTarget]::User)

    # Persist INCLUDE and LIB to search vcpkg headers/libraries using standard MSVC build scripts
    $vcpkgInclude = "$vcpkgRoot\installed\x64-windows\include"
    $vcpkgLib = "$vcpkgRoot\installed\x64-windows\lib"

    $userInclude = [System.Environment]::GetEnvironmentVariable("INCLUDE", [System.EnvironmentVariableTarget]::User)
    if ($userInclude -notlike "*$vcpkgInclude*") {
        $userInclude = if ($userInclude) { "$vcpkgInclude;$userInclude" } else { $vcpkgInclude }
        [System.Environment]::SetEnvironmentVariable("INCLUDE", $userInclude, [System.EnvironmentVariableTarget]::User)
    }

    $userLib = [System.Environment]::GetEnvironmentVariable("LIB", [System.EnvironmentVariableTarget]::User)
    if ($userLib -notlike "*$vcpkgLib*") {
        $userLib = if ($userLib) { "$vcpkgLib;$userLib" } else { $vcpkgLib }
        [System.Environment]::SetEnvironmentVariable("LIB", $userLib, [System.EnvironmentVariableTarget]::User)
    }

    # Prepend bin folders to PATH (User level)
    $userPath = [System.Environment]::GetEnvironmentVariable("Path", [System.EnvironmentVariableTarget]::User)
    $pathsToAdd = @("$qtRoot\bin", "$vcpkgRoot\installed\x64-windows\bin", $llvmPath)
    foreach ($p in $pathsToAdd) {
        if ($userPath -notlike "*$p*") {
            $userPath = "$p;$userPath"
        }
    }
    [System.Environment]::SetEnvironmentVariable("Path", $userPath, [System.EnvironmentVariableTarget]::User)

    # Set session variables so they are available immediately
    $env:QT_ROOT_DIR = $qtRoot
    $env:QMAKE = "$qtRoot\bin\qmake.exe"
    $env:VCPKG_ROOT = $vcpkgRoot
    $env:PKG_CONFIG_PATH = "$vcpkgRoot\installed\x64-windows\lib\pkgconfig"
    $env:FFMPEG_DIR = "$vcpkgRoot\installed\x64-windows"
    $env:LIBCLANG_PATH = $llvmPath
    $env:INCLUDE = if ($env:INCLUDE) { "$vcpkgInclude;$env:INCLUDE" } else { $vcpkgInclude }
    $env:LIB = if ($env:LIB) { "$vcpkgLib;$env:LIB" } else { $vcpkgLib }
    $env:Path = "$qtRoot\bin;$vcpkgRoot\installed\x64-windows\bin;$llvmPath;$env:Path"

    Write-Host "`nEnvironment Setup Completed Successfully!" -ForegroundColor Green
    Write-Host "IMPORTANT: Please close this terminal and open a new one (or restart your IDE) to apply the persistent environment variables." -ForegroundColor Green
}

# --- Load Environment Variables ---
function Load-Environment {
    # Check user environment registry if session variables aren't set yet
    if (-not $env:QT_ROOT_DIR) {
        $env:QT_ROOT_DIR = [System.Environment]::GetEnvironmentVariable("QT_ROOT_DIR", [System.EnvironmentVariableTarget]::User)
    }
    if (-not $env:QMAKE) {
        $env:QMAKE = [System.Environment]::GetEnvironmentVariable("QMAKE", [System.EnvironmentVariableTarget]::User)
    }
    if (-not $env:VCPKG_ROOT) {
        $env:VCPKG_ROOT = [System.Environment]::GetEnvironmentVariable("VCPKG_ROOT", [System.EnvironmentVariableTarget]::User)
    }
    if (-not $env:PKG_CONFIG_PATH) {
        $env:PKG_CONFIG_PATH = [System.Environment]::GetEnvironmentVariable("PKG_CONFIG_PATH", [System.EnvironmentVariableTarget]::User)
    }
    if (-not $env:FFMPEG_DIR) {
        $env:FFMPEG_DIR = [System.Environment]::GetEnvironmentVariable("FFMPEG_DIR", [System.EnvironmentVariableTarget]::User)
    }
    if (-not $env:LIBCLANG_PATH) {
        $env:LIBCLANG_PATH = [System.Environment]::GetEnvironmentVariable("LIBCLANG_PATH", [System.EnvironmentVariableTarget]::User)
    }

    # Standard Fallback values if not set
    if (-not $env:QT_ROOT_DIR) { $env:QT_ROOT_DIR = "C:\Qt\6.7.3\msvc2019_64" }
    if (-not $env:QMAKE) { $env:QMAKE = "$env:QT_ROOT_DIR\bin\qmake.exe" }
    if (-not $env:VCPKG_ROOT) { $env:VCPKG_ROOT = "C:\vcpkg" }
    if (-not $env:PKG_CONFIG_PATH) { $env:PKG_CONFIG_PATH = "$env:VCPKG_ROOT\installed\x64-windows\lib\pkgconfig" }
    if (-not $env:FFMPEG_DIR) { $env:FFMPEG_DIR = "$env:VCPKG_ROOT\installed\x64-windows" }
    if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin" }

    # Load INCLUDE and LIB if they are in User environment but not in session
    if (-not $env:INCLUDE) {
        $env:INCLUDE = [System.Environment]::GetEnvironmentVariable("INCLUDE", [System.EnvironmentVariableTarget]::User)
    }
    if (-not $env:LIB) {
        $env:LIB = [System.Environment]::GetEnvironmentVariable("LIB", [System.EnvironmentVariableTarget]::User)
    }

    # Ensure vcpkg headers and libraries are in session INCLUDE and LIB
    $vcpkgInclude = "$env:VCPKG_ROOT\installed\x64-windows\include"
    $vcpkgLib = "$env:VCPKG_ROOT\installed\x64-windows\lib"
    if ($env:INCLUDE -notlike "*$vcpkgInclude*") {
        $env:INCLUDE = if ($env:INCLUDE) { "$vcpkgInclude;$env:INCLUDE" } else { $vcpkgInclude }
    }
    if ($env:LIB -notlike "*$vcpkgLib*") {
        $env:LIB = if ($env:LIB) { "$vcpkgLib;$env:LIB" } else { $vcpkgLib }
    }

    # Ensure Qt, vcpkg, and LLVM bins are in the Session PATH so compiled binaries can run
    $qtBin = "$env:QT_ROOT_DIR\bin"
    $vcpkgBin = "$env:VCPKG_ROOT\installed\x64-windows\bin"
    $llvmBin = $env:LIBCLANG_PATH
    if ($env:Path -notlike "*$qtBin*") { $env:Path = "$qtBin;$env:Path" }
    if ($env:Path -notlike "*$vcpkgBin*") { $env:Path = "$vcpkgBin;$env:Path" }
    if ($env:Path -notlike "*$llvmBin*") { $env:Path = "$llvmBin;$env:Path" }

    # Configure Clang include paths dynamically for bindgen (fixes errno.h/stddef.h not found)
    if (-not $env:BINDGEN_EXTRA_CLANG_ARGS) {
        $includePaths = @()
        
        # 1. Try using the existing INCLUDE environment variable (if running in Developer PowerShell)
        if ($env:INCLUDE) {
            foreach ($path in $env:INCLUDE -split ';') {
                if ($path.Trim() -and (Test-Path $path)) {
                    $includePaths += $path.Trim()
                }
            }
        }
        
        # 2. If INCLUDE is empty (standard PowerShell), dynamically discover Visual Studio and Windows SDK paths
        if ($includePaths.Count -eq 0) {
            $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
            if (Test-Path $vsWhere) {
                $vsPath = & $vsWhere -latest -property installationPath
                if ($vsPath) {
                    $msvcBase = Join-Path $vsPath "VC\Tools\MSVC"
                    if (Test-Path $msvcBase) {
                        $msvcVersion = Get-ChildItem -Path $msvcBase -Directory | Sort-Object Name -Descending | Select-Object -First 1
                        if ($msvcVersion) {
                            $msvcInclude = Join-Path $msvcVersion.FullName "include"
                            if (Test-Path $msvcInclude) { $includePaths += $msvcInclude }
                        }
                    }
                }
            }
            
            $sdkBase = "C:\Program Files (x86)\Windows Kits\10\Include"
            if (Test-Path $sdkBase) {
                $sdkVersion = Get-ChildItem -Path $sdkBase -Directory | Sort-Object Name -Descending | Select-Object -First 1
                if ($sdkVersion) {
                    $subfolders = @("ucrt", "shared", "um", "winrt")
                    foreach ($sub in $subfolders) {
                        $p = Join-Path $sdkVersion.FullName $sub
                        if (Test-Path $p) { $includePaths += $p }
                    }
                }
            }
        }
        
        # 3. Export to BINDGEN_EXTRA_CLANG_ARGS
        if ($includePaths.Count -gt 0) {
            $clangArgs = @()
            foreach ($path in ($includePaths | Select-Object -Unique)) {
                $clangArgs += "-I`"$path`""
            }
            $env:BINDGEN_EXTRA_CLANG_ARGS = $clangArgs -join ' '
        }
    }

    # Fix legacy CMake compatibility issue for new CMake versions (e.g. CMake 4.0+)
    $env:CMAKE_POLICY_VERSION_MINIMUM = "3.5"
}

# Load environment on startup
Load-Environment

# --- Build & Run Actions ---
function Check-WebDeps {
    if (-not (Test-Path "web\node_modules")) {
        Write-Host "[Web] Frontend dependencies not found. Installing..." -ForegroundColor Yellow
        Push-Location web
        npm install
        Pop-Location
    } else {
        Write-Host "[Web] Frontend dependencies are already installed." -ForegroundColor Green
    }
}

function Run-Dev {
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " Starting Lunaris in DEV MODE (Concurrent)...     " -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    Check-WebDeps
    
    # Run the backend signaling server in a background job or separate process
    Write-Host "[Backend] Starting Rust signaling server (Debug)..." -ForegroundColor Yellow
    # We open a separate PowerShell window so they run concurrently and outputs are visible
    $serverProc = Start-Process powershell -ArgumentList "-NoExit", "-Command", "`$Host.UI.RawUI.WindowTitle='Lunaris Backend Server'; cargo run --manifest-path server/Cargo.toml --bin server" -PassThru
    $script:bgJobs += $serverProc

    # Run the React frontend Vite server
    Write-Host "[React Frontend] Starting Vite development server..." -ForegroundColor Yellow
    $webProc = Start-Process powershell -ArgumentList "-NoExit", "-Command", "`$Host.UI.RawUI.WindowTitle='Lunaris React Frontend'; cd web; npm run dev" -PassThru
    $script:bgJobs += $webProc
    
    Write-Host "`nDevelopment environment started in separate windows." -ForegroundColor Green
    Write-Host "Press Ctrl+C in this window to stop them, or close their respective windows." -ForegroundColor Green
    
    # Keep script alive to handle Ctrl+C cleanup
    try {
        while ($true) { Start-Sleep -Seconds 1 }
    } finally {
        Cleanup-Jobs
    }
}

function Build-Web {
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " Building Web Frontend Static Assets...          " -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    Check-WebDeps
    Push-Location web
    npm run build
    Pop-Location
}

function Build-Rust {
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " Building Rust Backend (Release)...              " -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    cargo build --release --workspace
}

function Build-All {
    Build-Web
    Build-Rust
    Write-Host "==================================================" -ForegroundColor Green
    Write-Host " All Release Builds Completed Successfully!      " -ForegroundColor Green
    Write-Host "==================================================" -ForegroundColor Green
}

function Run-Prod {
    Build-Web
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " Starting Lunaris in PRODUCTION MODE...          " -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    cargo run --release --manifest-path server/Cargo.toml --bin server
}

function Package-App {
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host " Packaging Lunaris Applications (Windows)...     " -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    Build-All

    # Package Server and Web Frontend
    Write-Host "Packaging Server..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Force -Path lunaris-server/web | Out-Null
    Copy-Item target/release/server.exe -Destination lunaris-server/
    Copy-Item -Recurse web/dist -Destination lunaris-server/web/dist
    if (Test-Path lunaris-server-windows.zip) { Remove-Item lunaris-server-windows.zip }
    Compress-Archive -Path lunaris-server/* -DestinationPath lunaris-server-windows.zip
    Write-Host "Server packaged -> lunaris-server-windows.zip" -ForegroundColor Green

    # Package Desktop QML Client
    Write-Host "Packaging Client Desktop..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Force -Path lunaris-client-desktop | Out-Null
    Copy-Item target/release/client-desktop.exe -Destination lunaris-client-desktop/
    
    # Run windeployqt to copy required Qt DLLs and plugins
    if (Test-Path "$env:QT_ROOT_DIR\bin\windeployqt.exe") {
        & "$env:QT_ROOT_DIR\bin\windeployqt.exe" --qmldir client-qml/qml --no-translations --compiler-runtime lunaris-client-desktop/client-desktop.exe
    } else {
        Write-Warning "windeployqt.exe not found. Skipping Qt DLL deployment."
    }

    # Copy vcpkg DLL dependencies (ffmpeg, opus, openssl)
    Copy-Item "$env:VCPKG_ROOT/installed/x64-windows/bin/*.dll" -Destination lunaris-client-desktop/ -ErrorAction SilentlyContinue
    if (Test-Path lunaris-client-desktop-windows.zip) { Remove-Item lunaris-client-desktop-windows.zip }
    Compress-Archive -Path lunaris-client-desktop/* -DestinationPath lunaris-client-desktop-windows.zip
    Write-Host "Desktop Client packaged -> lunaris-client-desktop-windows.zip" -ForegroundColor Green

    # Run NSIS Installer compiler if makensis is available
    if (Get-Command makensis -ErrorAction SilentlyContinue) {
        Write-Host "Compiling Installer via makensis..." -ForegroundColor Yellow
        & makensis client-desktop-installer.nsi
        Write-Host "Installer compiled -> lunaris-client-installer.exe" -ForegroundColor Green
    } else {
        Write-Host "makensis not found. Skipping installer compilation. (Install NSIS to build installer)" -ForegroundColor DarkYellow
    }

    # Package Host Agent
    Write-Host "Packaging Host Agent..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Force -Path lunaris-agent | Out-Null
    Copy-Item target/release/agent.exe -Destination lunaris-agent/
    Copy-Item "$env:VCPKG_ROOT/installed/x64-windows/bin/*.dll" -Destination lunaris-agent/ -ErrorAction SilentlyContinue
    if (Test-Path lunaris-agent-windows.zip) { Remove-Item lunaris-agent-windows.zip }
    Compress-Archive -Path lunaris-agent/* -DestinationPath lunaris-agent-windows.zip
    Write-Host "Host Agent packaged -> lunaris-agent-windows.zip" -ForegroundColor Green
}

# --- Command Line Flags Check ---
if ($args.Count -gt 0) {
    $arg = $args[0]
    switch ($arg) {
        "--dev"         { Run-Dev; exit 0 }
        "-dev"          { Run-Dev; exit 0 }
        "--prod"        { Run-Prod; exit 0 }
        "-prod"         { Run-Prod; exit 0 }
        "--build"       { Build-All; exit 0 }
        "-build"        { Build-All; exit 0 }
        "--build-web"   { Build-Web; exit 0 }
        "-build-web"    { Build-Web; exit 0 }
        "--build-rust"  { Build-Rust; exit 0 }
        "-build-rust"   { Build-Rust; exit 0 }
        "--setup"       { Setup-Environment; exit 0 }
        "-setup"        { Setup-Environment; exit 0 }
        "--package"     { Package-App; exit 0 }
        "-package"      { Package-App; exit 0 }
        Default {
            Write-Host "Usage: .\run.ps1 [--dev | --prod | --build | --build-web | --build-rust | --setup | --package]"
            exit 1
        }
    }
}

# --- Interactive Menu ---
while ($true) {
    # Check dependencies status
    $missing = Get-MissingDeps
    
    Clear-Host
    Write-Host "==================================================" -ForegroundColor Cyan
    Write-Host "           LUNARIS CONTROL PANEL (WINDOWS)        " -ForegroundColor Cyan
    Write-Host "==================================================" -ForegroundColor Cyan
    
    if ($missing.Count -gt 0) {
        Write-Host " WARNING: Missing dependencies detected:" -ForegroundColor Red
        foreach ($dep in $missing) {
            Write-Host "   - $dep" -ForegroundColor Red
        }
        Write-Host " Please choose Option 6 first to configure your environment.`n" -ForegroundColor Yellow
    } else {
        Write-Host " All build/run dependencies are met.`n" -ForegroundColor Green
    }
    
    Write-Host " 1) Run Development Mode (Server + Web Dev)"
    Write-Host " 2) Run Production Mode (Build Web + Start Server)"
    Write-Host " 3) Build All Release (Rust Workspace + Web)"
    Write-Host " 4) Build Rust Backend Only (Release)"
    Write-Host " 5) Build Web Frontend Only"
    Write-Host " 6) Install / Configure Windows Build Environment"
    Write-Host " 7) Package Applications (zip/installer)"
    Write-Host " 8) Exit"
    Write-Host "==================================================" -ForegroundColor Cyan
    
    $opt = Read-Host "Choose an option [1-8]"
    Write-Host ""

    switch ($opt) {
        "1" { Run-Dev }
        "2" { Run-Prod }
        "3" { Build-All }
        "4" { Build-Rust }
        "5" { Build-Web }
        "6" { Setup-Environment }
        "7" { Package-App }
        "8" { Write-Host "Goodbye!"; exit 0 }
        Default { Write-Host "Invalid option. Press any key to continue..."; [void][Console]::ReadKey($true) }
    }
    
    if ($opt -ne "1" -and $opt -ne "8") {
        Write-Host "`nPress any key to return to the menu..."
        [void][Console]::ReadKey($true)
    }
}
