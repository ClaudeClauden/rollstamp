<#
.SYNOPSIS
  Builds Rollstamp as a portable folder: rollstamp.exe + ExifTool + ImageMagick + licenses.
  Used locally (Build.bat) and by the GitHub release workflow.

.PARAMETER Latest
  Use the newest ExifTool/ImageMagick instead of the versions pinned in tools/versions.txt.
.PARAMETER Locked
  Build with --locked (reproducible; requires a committed Cargo.lock).
.PARAMETER NoRun
  Do not start Rollstamp at the end (CI).
.PARAMETER OutDir
  Output folder, default: dist\Rollstamp
#>
param(
    [switch]$Latest,
    [switch]$Locked,
    [switch]$NoRun,
    [string]$OutDir = ""
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
Set-Location -LiteralPath $root
if (-not $OutDir) { $OutDir = Join-Path $root "dist\Rollstamp" }
if (-not [IO.Path]::IsPathRooted($OutDir)) { $OutDir = Join-Path $root $OutDir }
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

function Step($text) {
    Write-Host ""
    Write-Host "==> $text" -ForegroundColor Cyan
}

function Fail($text) {
    Write-Host ""
    Write-Host "ERROR: $text" -ForegroundColor Red
    exit 1
}

function Download($url, $file) {
    Write-Host "    $url"
    Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue
    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        # "Wget" as user agent: SourceForge then delivers the file directly instead of a waiting page
        & curl.exe -L -s -f -A "Wget/1.21" -o $file $url
    } else {
        try { Invoke-WebRequest -UseBasicParsing -UserAgent "Wget/1.21" -Uri $url -OutFile $file -ErrorAction Stop } catch {}
    }
    return (Test-Path -LiteralPath $file) -and ((Get-Item -LiteralPath $file).Length -gt 100000)
}

function Expand-7z($archive, $dest) {
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    $sevenZip = Get-Command 7z.exe -ErrorAction SilentlyContinue
    if (-not $sevenZip) {
        $candidate = Join-Path $env:ProgramFiles "7-Zip\7z.exe"
        if (Test-Path -LiteralPath $candidate) { $sevenZip = $candidate }
    }
    if ($sevenZip) {
        & $sevenZip x -y "-o$dest" $archive | Out-Null
        return ($LASTEXITCODE -eq 0)
    }
    # Windows 11 ships a tar.exe that can read 7z archives
    & tar.exe -xf $archive -C $dest
    return ($LASTEXITCODE -eq 0)
}

try {
    # ------------------------------------------------------------ versions
    $versions = @{}
    Get-Content -LiteralPath (Join-Path $PSScriptRoot "versions.txt") | ForEach-Object {
        if ($_ -match '^\s*([A-Z_]+)\s*=\s*(\S+)') { $versions[$Matches[1]] = $Matches[2] }
    }
    $etVer = $versions['EXIFTOOL_VERSION']
    $imVer = $versions['IMAGEMAGICK_VERSION']
    if ($Latest) {
        try {
            $v = (Invoke-WebRequest -UseBasicParsing -Uri "https://exiftool.org/ver.txt" -ErrorAction Stop).Content.Trim()
            if ($v -match '^\d+\.\d+$') { $etVer = $v }
        } catch {}
        try {
            $rel = Invoke-RestMethod -Uri "https://api.github.com/repos/ImageMagick/ImageMagick/releases/latest" -Headers @{ 'User-Agent' = 'rollstamp-build' } -ErrorAction Stop
            if ($rel.tag_name) { $imVer = $rel.tag_name }
        } catch {}
    }
    Write-Host "ExifTool $etVer · ImageMagick $imVer"

    # ------------------------------------------------------------ Rust
    Step "Checking Rust"
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
        if (Test-Path -LiteralPath (Join-Path $cargoBin "cargo.exe")) {
            $env:Path = "$cargoBin;$env:Path"
        } else {
            Fail "Rust (cargo) not found. Install it from https://rustup.rs and sign out/in once."
        }
    }
    & rustc --version
    if ($LASTEXITCODE -ne 0) {
        & rustup default stable
        if ($LASTEXITCODE -ne 0) { Fail "Could not set up the Rust toolchain." }
    }

    # ------------------------------------------------------------ build
    Step "Building rollstamp.exe (first time 3-10 minutes, afterwards seconds)"
    $cargoArgs = @('build', '--release')
    if ($Locked) { $cargoArgs += '--locked' }
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        Fail ("Build failed (see messages above).`n" +
            "If it says 'link.exe not found': install 'Desktop development with C++' in the Visual Studio Installer.")
    }
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    Copy-Item -LiteralPath (Join-Path $root "target\release\rollstamp.exe") -Destination $OutDir -Force -ErrorAction Stop
    Set-Content -LiteralPath (Join-Path $OutDir "portable.txt") -Encoding UTF8 -Value @(
        "Rollstamp portable mode.",
        "As long as this file exists, settings and the thumbnail cache are stored in this folder.",
        "Delete it to use %APPDATA%\Rollstamp and %LOCALAPPDATA%\Rollstamp instead."
    )

    # ------------------------------------------------------------ ExifTool
    $etDir = Join-Path $OutDir "exiftool"
    $etExe = Join-Path $etDir "exiftool.exe"
    $haveEt = ""
    if (Test-Path -LiteralPath $etExe) { $haveEt = (& $etExe -ver 2>$null | Out-String).Trim() }
    if ($haveEt -ne $etVer) {
        Step "Downloading ExifTool $etVer"
        $zip = Join-Path $env:TEMP "exiftool-$($etVer)_64.zip"
        $ok = $false
        foreach ($u in @(
                "https://sourceforge.net/projects/exiftool/files/exiftool-$($etVer)_64.zip/download",
                "https://exiftool.org/exiftool-$($etVer)_64.zip")) {
            if (Download $u $zip) {
                $head = [IO.File]::ReadAllBytes($zip)
                if ($head[0] -eq 0x50 -and $head[1] -eq 0x4B) { $ok = $true; break }
            }
        }
        if (-not $ok) { Fail "ExifTool $etVer could not be downloaded." }
        $tmp = Join-Path $env:TEMP "rollstamp-exiftool"
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
        Expand-Archive -LiteralPath $zip -DestinationPath $tmp -Force -ErrorAction Stop
        $exe = Get-ChildItem -LiteralPath $tmp -Recurse -File | Where-Object { $_.Name -eq 'exiftool(-k).exe' } | Select-Object -First 1
        if (-not $exe) { Fail "exiftool(-k).exe not found in the download." }
        if (Test-Path -LiteralPath $etDir) { Remove-Item -LiteralPath $etDir -Recurse -Force }
        New-Item -ItemType Directory -Force -Path $etDir | Out-Null
        Copy-Item -LiteralPath $exe.FullName -Destination $etExe -Force -ErrorAction Stop
        Copy-Item -LiteralPath (Join-Path $exe.DirectoryName "exiftool_files") -Destination (Join-Path $etDir "exiftool_files") -Recurse -Force -ErrorAction Stop
        Remove-Item -LiteralPath $zip -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    $etCheck = (& $etExe -ver 2>$null | Out-String).Trim()
    if (-not $etCheck) { Fail "ExifTool does not start." }
    Write-Host "ExifTool $etCheck ready."

    # ------------------------------------------------------------ ImageMagick (portable)
    $imDir = Join-Path $OutDir "imagemagick"
    $imMarker = Join-Path $imDir "ROLLSTAMP-VERSION.txt"
    $haveIm = ""
    if (Test-Path -LiteralPath $imMarker) { $haveIm = (Get-Content -LiteralPath $imMarker -Raw).Trim() }
    if ($haveIm -ne $imVer) {
        Step "Downloading ImageMagick $imVer (portable)"
        $name = "ImageMagick-$imVer-portable-Q16-HDRI-x64.7z"
        $archive = Join-Path $env:TEMP $name
        if (-not (Download "https://github.com/ImageMagick/ImageMagick/releases/download/$imVer/$name" $archive)) {
            Fail "ImageMagick $imVer could not be downloaded."
        }
        $tmp = Join-Path $env:TEMP "rollstamp-imagemagick"
        if (Test-Path -LiteralPath $tmp) { Remove-Item -LiteralPath $tmp -Recurse -Force }
        if (-not (Expand-7z $archive $tmp)) {
            Fail "Could not extract $name. Please install 7-Zip (https://www.7-zip.org) and run again."
        }
        $magick = Get-ChildItem -LiteralPath $tmp -Recurse -File -Filter "magick.exe" | Select-Object -First 1
        if (-not $magick) { Fail "magick.exe not found in the ImageMagick package." }
        if (Test-Path -LiteralPath $imDir) { Remove-Item -LiteralPath $imDir -Recurse -Force }
        Copy-Item -LiteralPath $magick.DirectoryName -Destination $imDir -Recurse -Force -ErrorAction Stop
        Set-Content -LiteralPath $imMarker -Value $imVer -Encoding ASCII
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    $imCheck = (& (Join-Path $imDir "magick.exe") -version 2>$null | Select-Object -First 1)
    if (-not $imCheck) { Fail "ImageMagick does not start." }
    Write-Host "$imCheck"

    # ------------------------------------------------------------ licenses & docs
    Step "Copying licenses"
    foreach ($f in @("LICENSE", "THIRD-PARTY-NOTICES.md", "README.md", "README.de.md")) {
        $src = Join-Path $root $f
        if (Test-Path -LiteralPath $src) { Copy-Item -LiteralPath $src -Destination $OutDir -Force }
    }
    if (Get-Command cargo-about -ErrorAction SilentlyContinue) {
        & cargo about generate about.hbs -o (Join-Path $OutDir "THIRD-PARTY-LICENSES.html")
        if ($LASTEXITCODE -ne 0) { Write-Host "WARNING: cargo-about failed – THIRD-PARTY-LICENSES.html missing." -ForegroundColor Yellow }
    } else {
        Write-Host "cargo-about not installed – skipping THIRD-PARTY-LICENSES.html (cargo install cargo-about --locked)." -ForegroundColor Yellow
    }

    Step "Done: $OutDir"
    Write-Host "Portable: copy the whole folder anywhere and start rollstamp.exe." -ForegroundColor Green
    if (-not $NoRun) {
        Start-Process -FilePath (Join-Path $OutDir "rollstamp.exe") -WorkingDirectory $OutDir
    }
    exit 0
} catch {
    Fail $_.Exception.Message
}
