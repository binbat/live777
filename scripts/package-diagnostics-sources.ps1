param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("package", "verify")]
    [string]$Mode,
    [Parameter(Mandatory = $true)]
    [string]$Target,
    [Parameter(Mandatory = $true)]
    [string]$Version
)

$ErrorActionPreference = "Stop"
$Dist = "dist/diagnostics"
$BinaryStage = "live777-diagnostics-$Version-$Target"
$BinaryArchive = "$Dist/$BinaryStage.zip"
$SourceStage = "live777-diagnostics-sources-$Version-$Target"
$SourceDir = "$Dist/sources-work/$SourceStage"
$SourceArchive = "$Dist/$SourceStage.zip"
$Provenance = "$Dist/work/$BinaryStage/third-party/copied-files.tsv"
$SourceCount = 0

function Write-Utf8Lines {
    param([string]$Path, [string[]]$Lines)
    [System.IO.File]::WriteAllLines($Path, $Lines, [System.Text.UTF8Encoding]::new($false))
}

function Assert-NonemptyFile {
    param([string]$Path, [string]$Message)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf) -or (Get-Item -LiteralPath $Path).Length -eq 0) {
        throw $Message
    }
}

function Invoke-Git {
    param([string[]]$Arguments)
    & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Add-Source {
    param([string]$Kind, [string]$Name, [string]$SourceVersion, [string]$Path)
    Add-Content -LiteralPath "$SourceDir/SOURCES.tsv" -Value "$Kind`t$Name`t$SourceVersion`t$Path" -Encoding utf8
    $script:SourceCount++
}

function New-SourcePackage {
    Remove-Item -LiteralPath $SourceDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $SourceDir, $Dist -Force | Out-Null
    Write-Utf8Lines -Path "$SourceDir/SOURCES.tsv" -Lines @("kind`tname`tversion`tpath")
    Copy-Item -LiteralPath "licenses/diagnostics" -Destination "$SourceDir/licenses" -Recurse
    Assert-NonemptyFile -Path $Provenance -Message "Missing copied-library provenance: $Provenance"
    New-Item -ItemType Directory -Path "$SourceDir/third-party" -Force | Out-Null
    Copy-Item -LiteralPath $Provenance -Destination "$SourceDir/third-party/copied-files.tsv"
    Assert-NonemptyFile -Path $BinaryArchive -Message "Missing binary diagnostics archive: $BinaryArchive"

    $ProjectDir = Join-Path $SourceDir "project"
    New-Item -ItemType Directory -Path $ProjectDir -Force | Out-Null
    $ProjectCommit = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Unable to resolve the Live777 commit" }
    $ProjectZip = [System.IO.Path]::GetFullPath((Join-Path $ProjectDir "live777-$ProjectCommit.zip"))
    Invoke-Git -Arguments @("archive", "--format=zip", "--output=$ProjectZip", "HEAD")
    $ProjectPatch = [System.IO.Path]::GetFullPath((Join-Path $ProjectDir "tracked-changes.patch"))
    Invoke-Git -Arguments @("diff", "--binary", "HEAD", "--output=$ProjectPatch")
    $WorkspaceStatus = (& git status --porcelain) -join "`n"
    Write-Utf8Lines -Path (Join-Path $ProjectDir "workspace-status.txt") -Lines @($WorkspaceStatus)
    Add-Source -Kind project -Name live777 -SourceVersion $ProjectCommit -Path project

    $VcpkgRoot = if ($env:VCPKG_ROOT) { $env:VCPKG_ROOT } else { Join-Path $env:GITHUB_WORKSPACE "vcpkg" }
    $Downloads = Join-Path $VcpkgRoot "downloads"
    $Ports = Join-Path $VcpkgRoot "ports"
    $InstalledMetadata = Join-Path $VcpkgRoot "installed/vcpkg"
    if (-not (Test-Path -LiteralPath (Join-Path $VcpkgRoot "vcpkg.exe") -PathType Leaf)) {
        throw "vcpkg installation is missing: $VcpkgRoot"
    }
    $DownloadFiles = @(Get-ChildItem -LiteralPath $Downloads -Recurse -File)
    if ($DownloadFiles.Count -eq 0) { throw "vcpkg downloads are empty: $Downloads" }
    if (-not ($DownloadFiles | Where-Object { $_.Name -match "ffmpeg" -and $_.Length -gt 0 })) {
        throw "vcpkg downloads contain no nonempty FFmpeg source archive"
    }
    if (-not ($DownloadFiles | Where-Object { $_.Name -match "x264" -and $_.Length -gt 0 })) {
        throw "vcpkg downloads contain no nonempty x264 source archive"
    }
    if (-not (Test-Path -LiteralPath $Ports -PathType Container)) { throw "vcpkg ports are missing: $Ports" }
    if (-not (Test-Path -LiteralPath ".github/vcpkg-overlays/x264/portfile.cmake" -PathType Leaf)) {
        throw "The x264 overlay port is missing"
    }

    $VcpkgDestination = Join-Path $SourceDir "vcpkg"
    New-Item -ItemType Directory -Path $VcpkgDestination -Force | Out-Null
    Copy-Item -LiteralPath $Downloads -Destination $VcpkgDestination -Recurse
    Copy-Item -LiteralPath $Ports -Destination $VcpkgDestination -Recurse
    Copy-Item -LiteralPath ".github/vcpkg-overlays" -Destination (Join-Path $VcpkgDestination "overlays") -Recurse
    Copy-Item -LiteralPath $InstalledMetadata -Destination (Join-Path $VcpkgDestination "installed") -Recurse
    $VcpkgCommit = (& git -C $VcpkgRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) { throw "Unable to resolve the vcpkg revision" }
    $VcpkgPatch = [System.IO.Path]::GetFullPath((Join-Path $VcpkgDestination "tracked-changes.patch"))
    Invoke-Git -Arguments @("-C", $VcpkgRoot, "diff", "--binary", "HEAD", "--output=$VcpkgPatch")
    $VcpkgStatus = (& git -C $VcpkgRoot status --porcelain) -join "`n"
    Write-Utf8Lines -Path (Join-Path $VcpkgDestination "workspace-status.txt") -Lines @($VcpkgStatus)
    Write-Utf8Lines -Path (Join-Path $VcpkgDestination "revision.tsv") -Lines @("commit`t$VcpkgCommit", "triplet`tx64-windows")
    Add-Source -Kind vcpkg-sources -Name downloads -SourceVersion $VcpkgCommit -Path vcpkg/downloads
    Add-Source -Kind vcpkg-ports -Name ports -SourceVersion $VcpkgCommit -Path vcpkg/ports
    Add-Source -Kind vcpkg-overlay -Name overlays -SourceVersion $ProjectCommit -Path vcpkg/overlays
    Add-Source -Kind vcpkg-metadata -Name installed -SourceVersion $VcpkgCommit -Path vcpkg/installed

    Write-Utf8Lines -Path "$SourceDir/REBUILD.md" -Lines @(
        "# Rebuilding the corresponding diagnostics sources",
        "",
        "MANIFEST.tsv binds this archive to the binary diagnostics archive by target, version and SHA-256.",
        "The project/ zip is the Live777 source at $ProjectCommit. Apply tracked-changes.patch if non-empty.",
        "Build whipsynth, whepprobe and livewrk with Cargo using --features=rsmpeg and the manifest target.",
        "",
        "## vcpkg",
        "",
        "Clone https://github.com/microsoft/vcpkg and check out $VcpkgCommit.",
        "Restore vcpkg/downloads and vcpkg/ports over that checkout, apply vcpkg/tracked-changes.patch if non-empty,",
        "and use vcpkg/overlays as VCPKG_OVERLAY_PORTS. vcpkg/installed preserves the exact package versions and status.",
        "Run the vcpkg install command from the included Live777 .github/actions/install-ffmpeg/action.yml,",
        "pointing --downloads-root at the restored downloads so the cached source archives are used rather than",
        "silently selecting newer sources. Bootstrap vcpkg at the recorded revision before building.",
        "The downloads include the source archives for the copied FFmpeg, codec and other vcpkg DLLs.",
        "",
        "## Microsoft runtime",
        "",
        "The unmodified Microsoft Visual C++ runtime DLLs are proprietary redistributables, not vcpkg source.",
        "See licenses/MSVC-RUNTIME-NOTICE.md for Microsoft's redistribution terms and official download location."
    )

    $BinaryHash = (Get-FileHash -LiteralPath $BinaryArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($SourceCount -le 0) { throw "Refusing to create an empty source package" }
    Write-Utf8Lines -Path "$SourceDir/MANIFEST.tsv" -Lines @(
        "format`t1",
        "target`t$Target",
        "version`t$Version",
        "binary_archive`t$([System.IO.Path]::GetFileName($BinaryArchive))",
        "binary_sha256`t$BinaryHash",
        "source_count`t$SourceCount",
        "sources`tSOURCES.tsv",
        "rebuild`tREBUILD.md"
    )

    Remove-Item -LiteralPath $SourceArchive -Force -ErrorAction SilentlyContinue
    $SourceParent = Split-Path -Parent $SourceDir
    $SourceArchiveFull = [System.IO.Path]::GetFullPath($SourceArchive)
    Push-Location $SourceParent
    try {
        Compress-Archive -LiteralPath $SourceStage -DestinationPath $SourceArchiveFull
    }
    finally {
        Pop-Location
    }
    Assert-NonemptyFile -Path $SourceArchive -Message "Source archive is empty"
    if ($env:GITHUB_OUTPUT) {
        Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "source_archive=$SourceArchive"
    }
}

function Read-Manifest {
    param([string]$Path)
    $Manifest = @{}
    foreach ($Line in Get-Content -LiteralPath $Path) {
        if ([string]::IsNullOrWhiteSpace($Line)) { continue }
        $Parts = $Line -split "`t", 2
        if ($Parts.Count -eq 2) { $Manifest[$Parts[0]] = $Parts[1] }
    }
    return $Manifest
}

function Test-SourcePackage {
    Assert-NonemptyFile -Path $SourceArchive -Message "Missing source archive: $SourceArchive"
    Assert-NonemptyFile -Path $BinaryArchive -Message "Missing binary archive: $BinaryArchive"
    $Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $Temporary | Out-Null
    try {
        Expand-Archive -LiteralPath $SourceArchive -DestinationPath $Temporary
        $Bundle = Join-Path $Temporary $SourceStage
        foreach ($Required in @("MANIFEST.tsv", "SOURCES.tsv", "REBUILD.md", "third-party/copied-files.tsv")) {
            Assert-NonemptyFile -Path (Join-Path $Bundle $Required) -Message "Source archive is missing $Required"
        }
        $Manifest = Read-Manifest -Path (Join-Path $Bundle "MANIFEST.tsv")
        foreach ($Required in @("target", "version", "binary_archive", "binary_sha256", "source_count")) {
            if (-not $Manifest.ContainsKey($Required)) { throw "Source manifest has no $Required" }
        }
        if ($Manifest.target -ne $Target -or $Manifest.version -ne $Version) { throw "Source target or version mismatch" }
        if ($Manifest.binary_archive -ne [System.IO.Path]::GetFileName($BinaryArchive)) { throw "Binary archive name mismatch" }
        $BinaryHash = (Get-FileHash -LiteralPath $BinaryArchive -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($Manifest.binary_sha256 -ne $BinaryHash) { throw "Binary SHA-256 does not match source manifest" }
        if ([int]$Manifest.source_count -le 0) { throw "Source manifest has no sources" }

        foreach ($Line in Get-Content -LiteralPath (Join-Path $Bundle "SOURCES.tsv")) {
            $Parts = $Line -split "`t"
            if ($Parts.Count -ne 4 -or $Parts[0] -eq "kind") { continue }
            $SourcePath = $Parts[3]
            if ([System.IO.Path]::IsPathRooted($SourcePath) -or $SourcePath.Contains("..")) { throw "Unsafe source path: $SourcePath" }
            $Resolved = Join-Path $Bundle $SourcePath
            if (Test-Path -LiteralPath $Resolved -PathType Leaf) {
                Assert-NonemptyFile -Path $Resolved -Message "Manifest source is empty: $SourcePath"
            }
            elseif (Test-Path -LiteralPath $Resolved -PathType Container) {
                if (@(Get-ChildItem -LiteralPath $Resolved -Recurse -File).Count -eq 0) {
                    throw "Manifest source directory is empty: $SourcePath"
                }
            }
            else {
                throw "Manifest source is missing: $SourcePath"
            }
        }
        if (@(Get-ChildItem -LiteralPath (Join-Path $Bundle "project") -Filter "live777-*.zip" -File).Count -eq 0) {
            throw "Live777 project source is missing"
        }
        $Downloads = @(Get-ChildItem -LiteralPath (Join-Path $Bundle "vcpkg/downloads") -Recurse -File)
        if (-not ($Downloads | Where-Object { $_.Name -match "ffmpeg" -and $_.Length -gt 0 }) -or
            -not ($Downloads | Where-Object { $_.Name -match "x264" -and $_.Length -gt 0 })) {
            throw "Verified source archive lacks a nonempty FFmpeg or x264 source cache"
        }
    }
    finally {
        Remove-Item -LiteralPath $Temporary -Recurse -Force -ErrorAction SilentlyContinue
    }
}

switch ($Mode) {
    "package" { New-SourcePackage }
    "verify" { Test-SourcePackage }
}
