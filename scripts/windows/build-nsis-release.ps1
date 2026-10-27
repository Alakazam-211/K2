# build-nsis-release.ps1: build the K2 Windows NSIS installer (Alpha) on the
# Windows build box. Run by scripts/windows-nsis-build.sh over ssh, from the
# release tag's checkout; never by hand on a box with signing keys.
#
# The installer is NOT signed here: no signing keys live on a build box.
# The Mac that runs release.sh minisigns it for the updater
# (scripts/windows-patch-latest-json.sh). Authenticode is not done (Alpha).
#
# Mirrors macOS release.sh Step 2 / 2.5: the web UI, a release k2-daemon
# staged as a Tauri externalBin next to frpc, then `tauri build --bundles
# nsis`. Prints ALL_OK and INSTALLER=<path> on success.
#
# Defaults are K2-WIN-AX41's layout (everything on the D: Dev Drive).
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Tree = 'D:\src\K2',
    [string]$Target = 'D:\target',
    [string]$CargoHome = 'D:\cargo',
    [string]$RustupHome = 'D:\rustup',
    [string]$Temp = 'D:\tmp'
)
$ErrorActionPreference = 'Stop'
$Triple = 'x86_64-pc-windows-msvc'

function Fail([string]$msg) {
    Write-Output "FATAL: $msg"
    exit 1
}

function Run([string]$name, [scriptblock]$cmd) {
    Write-Output "=== $name ($(Get-Date -Format T))"
    & $cmd
    if ($LASTEXITCODE -ne 0) { Fail "$name failed (exit $LASTEXITCODE)" }
}

# A clean environment: no K2_* / K2SO_* from the ssh session, and no
# updater key even if someone set one (the build must not sign).
Get-ChildItem Env: | Where-Object { $_.Name -match '^(K2(SO)?_|TAURI_SIGNING_)' } |
    ForEach-Object { Remove-Item "Env:$($_.Name)" }
$env:CARGO_HOME = $CargoHome
$env:RUSTUP_HOME = $RustupHome
$env:CARGO_TARGET_DIR = $Target
$env:TEMP = $Temp
$env:TMP = $Temp
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
$env:CMAKE_GENERATOR = 'Ninja'
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
$env:CARGO_TERM_COLOR = 'never'
$machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
$user = [Environment]::GetEnvironmentVariable('Path', 'User')
$env:Path = "$CargoHome\bin;C:\Program Files\Git\cmd;C:\Program Files\LLVM\bin;C:\Program Files\CMake\bin;$machine;$user"

Set-Location $Tree
Write-Output "tree=$Tree target=$Target version=$Version"

# The checkout must be the version we were asked for (Tauri names the
# installer from it).
$pkg = (Get-Content package.json -Raw | ConvertFrom-Json).version
$cargo = (Select-String -Path src-tauri\Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if ($pkg -ne $Version -or $cargo -ne $Version) {
    Fail "checkout is package.json=$pkg src-tauri/Cargo.toml=$cargo, expected $Version"
}

$bin = Join-Path $Tree 'src-tauri\binaries'
$frpc = Join-Path $bin "frpc-$Triple.exe"
if (-not (Test-Path $frpc)) { Fail "frpc sidecar missing at $frpc (windows-nsis-build.sh stages it)" }

$nsisDir = Join-Path $Target 'release\bundle\nsis'
$setup = Join-Path $nsisDir "K2_${Version}_x64-setup.exe"
# A stale installer of any version must never pass for this one.
if (Test-Path $nsisDir) { Get-ChildItem $nsisDir -Filter 'K2_*_x64-setup.exe' | Remove-Item -Force }

Run 'bun install' { bun install --frozen-lockfile }
Run 'vite:build' { bun run vite:build }
Run 'cargo build k2-daemon' { cargo build --release -p k2-daemon }
$daemon = Join-Path $Target 'release\k2-daemon.exe'
if (-not (Test-Path $daemon)) { Fail "k2-daemon.exe missing at $daemon" }
Copy-Item -Force $daemon (Join-Path $bin "k2-daemon-$Triple.exe")

# beforeBuildCommand emptied: vite already ran above. A config file, not an
# inline JSON argument (PowerShell 5.1 mangles quotes in native arguments).
$override = Join-Path $Temp 'k2-nsis-build-override.json'
[IO.File]::WriteAllText($override, '{"build":{"beforeBuildCommand":""}}')
Run 'tauri build nsis' { bunx tauri build --bundles nsis --config $override }

foreach ($peer in 'k2.exe', 'k2-daemon.exe', 'frpc.exe') {
    if (-not (Test-Path (Join-Path $Target "release\$peer"))) { Fail "$peer missing from $Target\release after tauri build" }
}
if (-not (Test-Path $setup)) {
    Get-ChildItem $nsisDir -ErrorAction SilentlyContinue | Format-Table Name, Length | Out-String | Write-Output
    Fail "expected installer missing: $setup"
}
$len = (Get-Item $setup).Length
Write-Output "INSTALLER=$setup ($len bytes)"
Write-Output 'ALL_OK'
exit 0
