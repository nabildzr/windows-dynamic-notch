<#
.SYNOPSIS
    Builds Windows Dynamic Notch inside Docker (no local Rust toolchain needed).
.EXAMPLE
    .\build.ps1            # optimized release build -> dist\windows-dynamic-notch.exe
    .\build.ps1 check      # cargo check
    .\build.ps1 clippy     # lints, warnings are errors
    .\build.ps1 test       # builds the unit tests and runs them on this machine
#>
param([ValidateSet("release", "devrel", "installer", "check", "clippy", "test")][string]$Mode = "release")

# Exit codes are checked explicitly: cargo reports progress on stderr, which
# Windows PowerShell would otherwise turn into terminating errors.
# The executable cannot be replaced while it is running.
if ($Mode -in @("release", "devrel", "installer")) {
    Get-Process windows-dynamic-notch -ErrorAction SilentlyContinue | Stop-Process -Force
}
docker build -q -t notch-builder:local "$PSScriptRoot\docker" | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Error "docker build failed"; exit $LASTEXITCODE }
docker run --rm `
    -v "${PSScriptRoot}:/src" `
    -v notch-cargo-registry:/usr/local/cargo/registry `
    -v notch-target:/target `
    notch-builder:local sh /src/docker/build.sh $Mode
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if ($Mode -eq "test") {
    & "$PSScriptRoot\dist\notch-tests.exe"
}
exit $LASTEXITCODE
