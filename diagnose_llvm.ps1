# Lunaris LLVM / libclang & vcpkg Diagnostic Script

Write-Host "==================================================" -ForegroundColor Cyan
Write-Host "         LUNARIS ENV DIAGNOSTIC (WINDOWS)         " -ForegroundColor Cyan
Write-Host "==================================================" -ForegroundColor Cyan

# 1. Check Env Variables
Write-Host "`n[1] Environment Variables:" -ForegroundColor Yellow
Write-Host "  LIBCLANG_PATH: $env:LIBCLANG_PATH"
Write-Host "  VCPKG_ROOT: $env:VCPKG_ROOT"
Write-Host "  PATH contains LLVM: $( if ($env:Path -like '*LLVM*') { $true } else { $false } )"

# 2. Check libclang.dll location
$targetPaths = @(
    "C:\Program Files\LLVM\bin\libclang.dll",
    "C:\Program Files\LLVM\bin\clang.dll"
)

if ($env:LIBCLANG_PATH) {
    $targetPaths += Join-Path $env:LIBCLANG_PATH "libclang.dll"
    $targetPaths += Join-Path $env:LIBCLANG_PATH "clang.dll"
}

Write-Host "`n[2] Checking LLVM files on disk:" -ForegroundColor Yellow
foreach ($path in ($targetPaths | Select-Object -Unique)) {
    Write-Host "  Checking: $path"
    if (Test-Path $path) {
        $item = Get-Item $path
        Write-Host "    - Exists: Yes" -ForegroundColor Green
        Write-Host "    - Size: $($item.Length) bytes ($([Math]::Round($item.Length / 1MB, 2)) MB)"
        Write-Host "    - Last Modified: $($item.LastWriteTime)"
        
        # Test if file is empty
        if ($item.Length -eq 0) {
            Write-Host "    - WARNING: File is 0 bytes (empty/corrupted placeholder)!" -ForegroundColor Red
        } else {
            # Try to read the DOS header
            try {
                $bytes = [System.IO.File]::ReadAllBytes($path)
                if ($bytes.Length -ge 4) {
                    $hex = [System.BitConverter]::ToString($bytes[0..3])
                    $ascii = [System.Text.Encoding]::ASCII.GetString($bytes[0..3])
                    Write-Host "    - First 4 bytes (Hex): $hex"
                    Write-Host "    - First 4 bytes (ASCII): $ascii"
                    if ($hex -eq "4D-5A") {
                        Write-Host "    - Signature: MZ (Valid PE executable/DLL)" -ForegroundColor Green
                    } else {
                        Write-Host "    - WARNING: Invalid signature! Expected MZ (4D-5A)" -ForegroundColor Red
                    }
                }
            } catch {
                Write-Host "    - ERROR: Could not read file content! Exception: $_" -ForegroundColor Red
            }
        }
    } else {
        Write-Host "    - Exists: No" -ForegroundColor DarkGray
    }
}

# 3. Check clang execution
Write-Host "`n[3] Checking clang executable:" -ForegroundColor Yellow
$clangCmd = Get-Command clang -ErrorAction SilentlyContinue
if ($clangCmd) {
    Write-Host "  clang command found at: $($clangCmd.Source)" -ForegroundColor Green
    try {
        $version = & clang --version
        Write-Host "  clang --version output:"
        Write-Host "  $($version -join "`n  ")"
    } catch {
        Write-Host "  ERROR: Failed to run clang --version: $_" -ForegroundColor Red
    }
} else {
    Write-Host "  clang executable not found in PATH" -ForegroundColor Red
}

# 4. Search for other LLVM installations
Write-Host "`n[4] Searching for other LLVM installations on C: drive:" -ForegroundColor Yellow
$searchPaths = @(
    "C:\Program Files\LLVM",
    "C:\Program Files (x86)\LLVM",
    "C:\tools\llvm",
    "C:\Program Files\Microsoft Visual Studio"
)

foreach ($folder in $searchPaths) {
    if (Test-Path $folder) {
        Write-Host "  Found directory: $folder"
        $dlls = Get-ChildItem -Path $folder -Filter "libclang.dll" -Recurse -File -ErrorAction SilentlyContinue
        foreach ($dll in $dlls) {
            Write-Host "    - Found libclang.dll at: $($dll.FullName) ($([Math]::Round($dll.Length / 1MB, 2)) MB)" -ForegroundColor Gray
        }
    }
}

# 5. Check vcpkg status & packages
Write-Host "`n[5] Checking vcpkg status & packages:" -ForegroundColor Yellow
$vcpkgRoot = if ($env:VCPKG_ROOT) { $env:VCPKG_ROOT } else { "C:\vcpkg" }
if (Test-Path "$vcpkgRoot\vcpkg.exe") {
    Write-Host "  vcpkg root: $vcpkgRoot"
    
    # Git info
    if (Test-Path "$vcpkgRoot\.git") {
        Push-Location $vcpkgRoot
        $gitBranch = git status -b -s 2>$null
        $gitLog = git log -n 1 --oneline 2>$null
        Pop-Location
        Write-Host "  vcpkg git branch/status: $gitBranch"
        Write-Host "  vcpkg git commit: $gitLog"
    } else {
        Write-Host "  vcpkg directory is not a Git repository" -ForegroundColor Yellow
    }
    
    # List packages
    Write-Host "  Installed packages related to ffmpeg, opus, openssl:"
    & "$vcpkgRoot\vcpkg.exe" list | Where-Object { $_ -match "ffmpeg" -or $_ -match "opus" -or $_ -match "openssl" }
} else {
    Write-Host "  vcpkg.exe not found at $vcpkgRoot" -ForegroundColor Red
}

Write-Host "`n==================================================" -ForegroundColor Cyan
