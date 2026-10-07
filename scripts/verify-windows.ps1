# verify-windows.ps1 — disciplined first-run protocol for N58C.
# Run from the repo root on a Windows x86_64 host:
#     powershell -ExecutionPolicy Bypass -File scripts\verify-windows.ps1
#
# Staged so failures classify themselves: each check is tagged
# COMPILE / LINK / ABI / SEMANTIC / HARNESS. A blind implementation
# should fail early and cheaply before the corpus runs.
#
# Requires: cargo (Rust), clang + lld-link (LLVM or VS Build Tools),
# Windows SDK import libs (ucrt/kernel32/ws2_32 — any VS install).
$ErrorActionPreference = 'Continue'
# Child stdout is UTF-8 by contract — decode it as such so the
# non-ASCII argv comparison isn't corrupted by the console codepage.
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location (Join-Path $PSScriptRoot '..')
$ROOT = Get-Location
$OURC = Join-Path $ROOT 'target\debug\ourc.exe'
$OUR  = Join-Path $ROOT 'target\debug\our.exe'
$WORK = Join-Path $env:TEMP "mml-verify-$PID"
New-Item -ItemType Directory -Force -Path $WORK | Out-Null

$script:Pass = 0; $script:Fail = 0; $script:Skip = 0
function ok   { param($m) $script:Pass++; Write-Host "  PASS  $m" }
function bad  { param($m,$cat='SEMANTIC') $script:Fail++; Write-Host "  FAIL  [$cat] $m" }
function skip { param($m) $script:Skip++; Write-Host "  SKIP  $m" }

Write-Host "== phase 0: host + toolchain =="
Get-CimInstance Win32_OperatingSystem | ForEach-Object { "$($_.Caption) $($_.Version) $($_.OSArchitecture)" }
rustc --version
# Mirror pipeline.rs cc(): ourc drives whatever 'clang' resolves to.
$cc = Get-Command clang -ErrorAction SilentlyContinue
if ($cc) { Write-Host "resolved C driver: $($cc.Source)"; & clang --version | Select-Object -First 1 }
else { Write-Host "FAIL  [HARNESS] clang not on PATH"; exit 1 }
Get-Command lld-link -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "lld-link: $($_.Source)" }
Get-Command link -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "link.exe: $($_.Source)" }

Write-Host "== phase 1: the embarrassingly small step — runtime compiles alone =="
foreach ($f in 'mmlrt_win32.c', 'mmlrt_common.c') {
    $obj = Join-Path $WORK ($f -replace '\.c$', '.o')
    & clang -O2 -c "compiler\runtime\$f" -o $obj 2>&1 | Out-Null
    if ($LASTEXITCODE -eq 0 -and (Test-Path $obj)) { ok "clang -c $f" }
    else { bad "clang -c $f" 'COMPILE' }
}

Write-Host "== phase 2: toolchain build =="
cargo build -p ourc 2>&1 | Select-Object -Last 2
if (Test-Path $OURC) { ok "cargo build -p ourc" } else { bad "ourc.exe built" 'HARNESS'; exit 1 }

Write-Host "== phase 3: smoke — trivial binary links and runs =="
'fn main() -> i32 { return 0; }' | Set-Content (Join-Path $WORK 't.lang')
& $OURC (Join-Path $WORK 't.lang') -o (Join-Path $WORK 't') 2>&1 | Select-Object -Last 1
if (Test-Path (Join-Path $WORK 't.exe')) { ok "-o produced t.exe" } else { bad "t.exe exists" 'LINK' }
& (Join-Path $WORK 't.exe'); if ($LASTEXITCODE -eq 0) { ok "smoke exit 0" } else { bad "smoke exit $LASTEXITCODE" 'ABI' }

Write-Host "== phase 4: argv — ASCII then non-ASCII UTF-16 =="
@'
import std.io;
import std.process;
fn main(args: slice str) -> i32 {
    if args.len < 2 { return 1; }
    io.println(args[1]);
    return 42;
}
'@ | Set-Content (Join-Path $WORK 'argv.lang')
& $OURC (Join-Path $WORK 'argv.lang') -o (Join-Path $WORK 'argv') | Out-Null
$out = & (Join-Path $WORK 'argv.exe') hello
if ($out -eq 'hello' -and $LASTEXITCODE -eq 42) { ok "argv ASCII" } else { bad "argv ASCII ($out, $LASTEXITCODE)" 'SEMANTIC' }
$out = & (Join-Path $WORK 'argv.exe') 'héllo-wörld-→'
if ($out -eq 'héllo-wörld-→' -and $LASTEXITCODE -eq 42) { ok "argv UTF-16->UTF-8" } else { bad "argv UTF-16->UTF-8 ($out)" 'SEMANTIC' }

Write-Host "== phase 5: env lookup through wide APIs =="
@'
import std.io;
import std.process;
fn main() -> i32 {
    match process.env("TEMP") {
        Ok(v) => { io.println("TEMP ok"); return 42; }
        Err(e) => { return 1; }
    }
}
'@ | Set-Content (Join-Path $WORK 'env.lang')
& $OURC (Join-Path $WORK 'env.lang') -o (Join-Path $WORK 'env') | Out-Null
& (Join-Path $WORK 'env.exe')
if ($LASTEXITCODE -eq 42) { ok "env TEMP via GetEnvironmentVariableW" } else { bad "env lookup" 'SEMANTIC' }

Write-Host "== phase 6: file I/O must be byte-preserving =="
@'
import std.fs;
import std.io;
import std.result;
fn go() -> result.Result<i32, i32> {
    let path: str = "mml-bin.dat";
    let w = fs.open(path.bytes.ptr, fs.o_write() | fs.o_create() | fs.o_trunc(), 420)?;
    let buf: [6]u8;
    buf[0] = 0x0D; buf[1] = 0x0A; buf[2] = 0x00;   // CRLF + NUL — text-mode poison
    buf[3] = 0xC3; buf[4] = 0xA9; buf[5] = 0x0D;   // é UTF-8 + stray CR
    fs.write(w, &buf[..], 6)?;
    fs.close(w)?;
    let r = fs.open(path.bytes.ptr, fs.o_read(), 0)?;
    let back: [64]u8;
    let n = fs.read(r, &mut back[..], 64)?;
    fs.close(r)?;
    if n != 6 { return result.Result<i32, i32>::Ok(1); }
    if back[0] != 0x0D || back[3] != 0xC3 { return result.Result<i32, i32>::Ok(2); }
    return result.Result<i32, i32>::Ok(42);
}
fn main() -> i32 {
    match go() {
        Ok(v) => { return v; }
        Err(e) => { return 100 + e; }
    }
}
'@ | Set-Content (Join-Path $WORK 'fio.lang')
& $OURC (Join-Path $WORK 'fio.lang') -o (Join-Path $WORK 'fio') | Out-Null
Push-Location $WORK; & (Join-Path $WORK 'fio.exe'); $rc = $LASTEXITCODE; Pop-Location
if ($rc -eq 42) { ok "binary-mode write+read round-trip" } else { bad "binary I/O (exit $rc)" 'SEMANTIC' }

Write-Host "== phase 7: sockets — net pins =="
foreach ($t in '514_net', '515_net_errors') {
    & $OURC "native-tests\$t.lang" -o (Join-Path $WORK $t) 2>&1 | Select-Object -Last 1
    & (Join-Path $WORK "$t.exe")
    if ($LASTEXITCODE -eq 42) { ok "$t (socket round-trip)" } else { bad "$t — exit $LASTEXITCODE" 'SEMANTIC' }
}

Write-Host "== phase 8: threads + mutexes + channels =="
# task spawn/join, mutex'd writers, cross-task channels — the Win32
# primitive mapping (CreateThread/SRWLOCK/CONDITION_VARIABLE) lives
# or dies here.
foreach ($t in @('native-tests\121_task_readers.lang',
                 'native-tests\127_task_result.lang',
                 'native-tests\156_mutex_two_writers.lang',
                 'native-tests\199_chan_stream.lang')) {
    if (Test-Path $t) {
        $n = [IO.Path]::GetFileNameWithoutExtension($t)
        & $OURC $t -o (Join-Path $WORK $n) 2>&1 | Select-Object -Last 1
        # 156_mutex_two_writers expects exit 3, the rest expect 42 —
        # read each pin's own @exit directive rather than hardcoding.
        $want = [int]((Select-String -Path $t -Pattern '@exit (-?\d+)').Matches.Groups[1].Value)
        & (Join-Path $WORK "$n.exe")
        if ($LASTEXITCODE -eq $want) { ok "$n (exit $want)" } else { bad "$n — exit $LASTEXITCODE want $want" 'SEMANTIC' }
    }
}

Write-Host "== phase 9: pin corpus + unit tests =="
cargo test --workspace 2>&1 | Tee-Object (Join-Path $WORK 'cargo.log') | Select-String 'test result|FAILED|failed' | Select-Object -Last 12
if ((Select-String -Path (Join-Path $WORK 'cargo.log') -Pattern '0 failed').Count -gt 0 -and
    -not (Select-String -Path (Join-Path $WORK 'cargo.log') -Pattern 'FAILED').Count) {
    ok "cargo test --workspace"
} else { bad "cargo test --workspace — see $WORK\cargo.log" 'HARNESS' }

Write-Host "== phase 10: real programs — build all =="
Get-ChildItem programs\*.lang | ForEach-Object {
    $n = $_.BaseName
    & $OURC $_.FullName -o (Join-Path $WORK $n) 2>&1 | Select-Object -Last 1
    if (Test-Path (Join-Path $WORK "$n.exe")) { ok "build $n" } else { bad "build $n" 'COMPILE' }
}
& $OURC programs\hexdump\main.lang -o (Join-Path $WORK 'hexdump') 2>&1 | Select-Object -Last 1
if (Test-Path (Join-Path $WORK 'hexdump.exe')) { ok "build hexdump" } else { bad "build hexdump" 'COMPILE' }

Write-Host "== phase 11: real programs — run =="
function Run-Expect($name, $want, $argv) {
    $p = Start-Process -FilePath (Join-Path $WORK "$name.exe") -ArgumentList $argv `
        -NoNewWindow -PassThru -Wait
    if ($p.ExitCode -eq $want) { ok "run $name" } else { bad "run $name — $($p.ExitCode) want $want" 'SEMANTIC' }
}
Run-Expect graph 42 @()
Run-Expect lru 42 @()
Run-Expect prodcons 0 @()
Run-Expect psum 0 @()
Run-Expect swapstress 42 @()
Run-Expect workq 42 @()
Run-Expect kvserver 42 @()          # selftest
# byte-precise fixture — Set-Content would append a CRLF
[IO.File]::WriteAllBytes((Join-Path $WORK 'in.txt'), [byte[]](0x69,0x6E,0x2E,0x74,0x78,0x74))
Run-Expect fcopy 0 @((Join-Path $WORK 'in.txt'), (Join-Path $WORK 'out.txt'))
$in_b = [IO.File]::ReadAllBytes((Join-Path $WORK 'in.txt'))
$out_b = [IO.File]::ReadAllBytes((Join-Path $WORK 'out.txt'))
if ([Linq.Enumerable]::SequenceEqual($in_b, $out_b)) { ok "fcopy byte-exact" } else { bad "fcopy content" 'SEMANTIC' }
Run-Expect ugrep 0 @('fixture', (Join-Path $WORK 'in.txt'))

Write-Host "== phase 12: live socket server =="
$hp = Start-Process -FilePath (Join-Path $WORK 'httpd.exe') -ArgumentList '18080' -NoNewWindow -PassThru
Start-Sleep -Seconds 1
try {
    $code = (curl.exe -s -o NUL -w '%{http_code}' -m 3 http://127.0.0.1:18080/ 2>$null)
    if ($code -match '^(200|400|404)$') { ok "httpd bind/listen/answer" } else { bad "httpd — '$code'" 'SEMANTIC' }
} finally { Stop-Process -Id $hp.Id -Force -ErrorAction SilentlyContinue }

Write-Host "== phase 13: project tool =="
Push-Location $WORK
& $OUR new proj 2>&1 | Select-Object -Last 1
Set-Location proj
& $OUR check 2>&1 | Out-Null; $c1 = $LASTEXITCODE
& $OUR test  2>&1 | Out-Null; $c2 = $LASTEXITCODE
& $OUR build 2>&1 | Out-Null; $c3 = $LASTEXITCODE
& $OUR run   2>&1 | Out-Null; $c4 = $LASTEXITCODE
Pop-Location
if ($c1 -eq 0 -and $c2 -eq 0 -and $c3 -eq 0 -and $c4 -eq 42) { ok "our new → check/test/build/run" }
else { bad "project tool — check=$c1 test=$c2 build=$c3 run=$c4" 'HARNESS' }

Write-Host "== phase 14: debug info (non-blocking) =="
& $OURC (Join-Path $WORK 't.lang') -g -o (Join-Path $WORK 't.dbg') | Out-Null
if (Test-Path (Join-Path $WORK 't.dbg.exe')) { ok "-g build links" } else { bad "-g build" 'LINK' }
$dd = Get-Command llvm-dwarfdump -ErrorAction SilentlyContinue
if ($dd) {
    & llvm-dwarfdump (Join-Path $WORK 't.dbg.exe') 2>&1 | Select-String 'DW_TAG' | Select-Object -First 1 | Out-Null
    if ($LASTEXITCODE -eq 0) { ok "DWARF/CodeView present" } else { skip "dwarfdump output unrecognized — check manually" }
} else { skip "no llvm-dwarfdump — check debug info manually" }

Write-Host ""
Write-Host "==== summary: $Pass passed, $Fail failed, $Skip skipped (artifacts in $WORK) ===="
exit ($Fail -ne 0)
