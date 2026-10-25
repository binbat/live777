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
    $CargoTargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
    $BuildDir = "$CargoTargetDir/$Target/release"
    $DllDir = Join-Path $env:GITHUB_WORKSPACE "vcpkg/installed/x64-windows/bin"

    Remove-Item -LiteralPath $StageDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    foreach ($Tool in $Tools) {
        Copy-Item -LiteralPath (Join-Path $BuildDir "$Tool.exe") -Destination (Join-Path $BinDir "$Tool.exe")
    }
    $Dlls = Get-ChildItem -LiteralPath $DllDir -Filter "*.dll" -File
    if ($Dlls.Count -eq 0) {
        throw "No FFmpeg runtime DLLs found in $DllDir"
    }
    Copy-Item -LiteralPath $Dlls.FullName -Destination $BinDir

    $RuntimePatterns = @("vcruntime140*.dll", "msvcp140*.dll", "concrt140*.dll", "vccorlib140*.dll")
    $System32 = Join-Path $env:SystemRoot "System32"
    $RuntimeDlls = @($RuntimePatterns | ForEach-Object {
        Get-ChildItem -LiteralPath $System32 -Filter $_ -File
    })
    if ($RuntimeDlls.Count -eq 0) {
        throw "No x64 MSVC runtime DLLs found in $System32"
    }
    Copy-Item -LiteralPath $RuntimeDlls.FullName -Destination $BinDir
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
