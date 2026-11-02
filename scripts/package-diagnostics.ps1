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
$Stage = "live777-diagnostics-$Version-$Target"
$Dist = "dist/diagnostics"
$Archive = "$Dist/$Stage.zip"
$Tools = @("whipsynth", "whepprobe", "livewrk")

function New-DiagnosticsPackage {
    $Work = Join-Path $Dist "work"
    $StageDir = Join-Path $Work $Stage
    $BinDir = Join-Path $StageDir "bin"
    $ThirdPartyDir = Join-Path $StageDir "third-party"
    $LicenseDir = Join-Path $StageDir "licenses"
    $CargoTargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
    $BuildDir = "$CargoTargetDir/$Target/release"
    $DllDir = Join-Path $env:GITHUB_WORKSPACE "vcpkg/installed/x64-windows/bin"

    Remove-Item -LiteralPath $StageDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $BinDir, $ThirdPartyDir, $LicenseDir -Force | Out-Null
    Copy-Item -Path "licenses/diagnostics/*" -Destination $LicenseDir -Recurse
    foreach ($Required in @("NOTICE.md", "GPL-2.0.txt", "LGPL-2.1.txt", "GPL-3.0.txt", "LGPL-3.0.txt", "Apache-2.0.txt", "FFmpeg-LICENSE.md")) {
        if ((Get-Item -LiteralPath (Join-Path $LicenseDir $Required)).Length -eq 0) {
            throw "Empty license notice: $Required"
        }
    }

    $Provenance = @()
    foreach ($Tool in $Tools) {
        Copy-Item -LiteralPath (Join-Path $BuildDir "$Tool.exe") -Destination (Join-Path $BinDir "$Tool.exe")
    }
    $Dlls = @(Get-ChildItem -LiteralPath $DllDir -Filter "*.dll" -File)
    if ($Dlls.Count -eq 0) {
        throw "No FFmpeg runtime DLLs found in $DllDir"
    }
    foreach ($Dll in $Dlls) {
        Copy-Item -LiteralPath $Dll.FullName -Destination $BinDir
        $Provenance += "bin/$($Dll.Name)`t$($Dll.FullName)"
    }

    $RuntimePatterns = @("vcruntime140*.dll", "msvcp140*.dll", "concrt140*.dll", "vccorlib140*.dll")
    $System32 = Join-Path $env:SystemRoot "System32"
    $RuntimeDlls = @($RuntimePatterns | ForEach-Object {
        Get-ChildItem -LiteralPath $System32 -Filter $_ -File
    })
    if ($RuntimeDlls.Count -eq 0) {
        throw "No x64 MSVC runtime DLLs found in $System32"
    }
    foreach ($Dll in $RuntimeDlls) {
        Copy-Item -LiteralPath $Dll.FullName -Destination $BinDir
        $Provenance += "bin/$($Dll.Name)`t$($Dll.FullName)"
    }
    [System.IO.File]::WriteAllLines((Join-Path $ThirdPartyDir "copied-files.tsv"), $Provenance, [System.Text.UTF8Encoding]::new($false))

    $ShareDir = Join-Path $env:GITHUB_WORKSPACE "vcpkg/installed/x64-windows/share"
    $VcpkgNotices = @(Get-ChildItem -LiteralPath $ShareDir -Recurse -Filter "copyright" -File)
    if ($VcpkgNotices.Count -eq 0) {
        throw "No vcpkg copyright notices found in $ShareDir"
    }
    $VcpkgLicenseDir = Join-Path $LicenseDir "vcpkg"
    New-Item -ItemType Directory -Path $VcpkgLicenseDir -Force | Out-Null
    foreach ($Notice in $VcpkgNotices) {
        Copy-Item -LiteralPath $Notice.FullName -Destination (Join-Path $VcpkgLicenseDir "$($Notice.Directory.Name)-copyright")
    }
    Copy-Item -LiteralPath "LICENSE" -Destination (Join-Path $StageDir "LICENSE")

    New-Item -ItemType Directory -Path $Dist -Force | Out-Null
    Remove-Item -LiteralPath $Archive -Force -ErrorAction SilentlyContinue
    Push-Location $Work
    try {
        Compress-Archive -LiteralPath $Stage -DestinationPath (Join-Path (Get-Location) "../$Stage.zip")
    }
    finally {
        Pop-Location
    }
    if ($env:GITHUB_OUTPUT) {
        Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "archive=$Archive"
    }
}

function Test-DiagnosticsPackage {
    $Temporary = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    $SavedEnvironment = @{}
    $Keep = @("SystemRoot", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "PATHEXT", "ComSpec", "OS")
    New-Item -ItemType Directory -Path $Temporary | Out-Null
    try {
        Expand-Archive -LiteralPath $Archive -DestinationPath $Temporary
        $Bundle = Join-Path $Temporary "relocated bundle"
        Move-Item -LiteralPath (Join-Path $Temporary $Stage) -Destination $Bundle
        $BinDir = Join-Path $Bundle "bin"
        foreach ($Required in @("NOTICE.md", "GPL-2.0.txt", "LGPL-2.1.txt", "GPL-3.0.txt", "LGPL-3.0.txt", "Apache-2.0.txt", "FFmpeg-LICENSE.md")) {
            $Notice = Join-Path $Bundle "licenses/$Required"
            if (-not (Test-Path -LiteralPath $Notice -PathType Leaf) -or (Get-Item -LiteralPath $Notice).Length -eq 0) {
                throw "The extracted diagnostics bundle is missing license notice $Required"
            }
        }
        $Provenance = Join-Path $Bundle "third-party/copied-files.tsv"
        if (-not (Test-Path -LiteralPath $Provenance -PathType Leaf) -or (Get-Item -LiteralPath $Provenance).Length -eq 0) {
            throw "The extracted diagnostics bundle has no copied-library provenance"
        }
        $VcpkgNotices = @(Get-ChildItem -LiteralPath (Join-Path $Bundle "licenses/vcpkg") -Filter "*-copyright" -File)
        if ($VcpkgNotices.Count -eq 0) {
            throw "The extracted diagnostics bundle has no vcpkg copyright notices"
        }
        $BundledRuntime = @(Get-ChildItem -LiteralPath $BinDir -Filter "*.dll" -File | Where-Object {
            $_.Name -match "^(vcruntime140|msvcp140|concrt140|vccorlib140).*\.dll$"
        })
        if ($BundledRuntime.Count -eq 0) {
            throw "The extracted diagnostics bundle has no MSVC runtime DLLs"
        }
        foreach ($RuntimeName in @("vcruntime140.dll", "msvcp140.dll")) {
            if (-not (Test-Path -LiteralPath (Join-Path $BinDir $RuntimeName) -PathType Leaf)) {
                throw "The extracted diagnostics bundle is missing $RuntimeName"
            }
        }

        Get-ChildItem Env: | ForEach-Object { $SavedEnvironment[$_.Name] = $_.Value }
        Get-ChildItem Env: | Where-Object { $Keep -notcontains $_.Name } | ForEach-Object {
            Remove-Item -LiteralPath "Env:$($_.Name)"
        }
        $env:PATH = $BinDir
        foreach ($Tool in $Tools) {
            & (Join-Path $BinDir "$Tool.exe") --version
            if ($LASTEXITCODE -ne 0) {
                throw "$Tool --version failed with exit code $LASTEXITCODE"
            }
        }
    }
    finally {
        if ($SavedEnvironment.Count -gt 0) {
            foreach ($Name in (Get-ChildItem Env:).Name) {
                Remove-Item -LiteralPath "Env:$Name"
            }
            foreach ($Entry in $SavedEnvironment.GetEnumerator()) {
                Set-Item -LiteralPath "Env:$($Entry.Key)" -Value $Entry.Value
            }
        }
        Remove-Item -LiteralPath $Temporary -Recurse -Force -ErrorAction SilentlyContinue
    }
}

switch ($Mode) {
    "package" { New-DiagnosticsPackage }
    "verify" { Test-DiagnosticsPackage }
}
