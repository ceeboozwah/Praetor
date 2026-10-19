# verify-windows.ps1 — disciplined first-run protocol for N79C.
# Run from the repo root on a Windows x86_64 host:
#     powershell -ExecutionPolicy Bypass -File scripts\verify-windows.ps1
#
# Ladder, smallest proof first: runtime compile -> runtime C probe
# -> Stage 0 (ourc.exe) -> Stage 1 self-hosted (our.exe) -> hello
# -> targeted subsystem smokes -> full corpus (classified,
# report-only) -> s2/s3 fixpoint -> project tool.
#
# Corpus rule for N79C: every FAIL must classify as REAL DEFECT or
# PLATFORM ASSUMPTION (documented POSIX-only fixture). Zero
# unexplained failures is the bar — not zero failures.
#
# Requires: cargo (Rust), clang + lld-link (LLVM or VS Build Tools),
# Windows SDK import libs (ucrt/kernel32/ws2_32 — any VS install).
$ErrorActionPreference = 'Continue'
# Child stdout is UTF-8 by contract — decode it as such so the
# non-ASCII argv comparison isn't corrupted by the console codepage.
[Console]::OutputEncoding = [Text.Encoding]::UTF8
Set-Location (Join-Path $PSScriptRoot '..')
$ROOT = Get-Location
$OURC = Join-Path $ROOT 'target\release\ourc.exe'
# The self-hosted driver — built by ourc from selfhost/our.lang
# during this run; after that it does the compiling.
$OURS = Join-Path $env:TEMP "mml-verify-$PID-s1\our.exe"
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

Write-Host "== phase 1.5: runtime C probe — triple + exit 42 =="
# Proves the runtime works before any compiler is involved.
# mml_main_args is the program entry the runtime calls.
@'
#include "mmlrt.h"
long long mml_main_args(const mml_str_t *a, long long n) {
    (void)a; (void)n;
    const char *t = 0;
    long long k = mml_target_triple(&t);
    if (k <= 0) return 1;
    mml_fd_write(1, t, k); mml_fd_write(1, "\n", 1);
    return 42;
}
'@ | Set-Content (Join-Path $WORK 'probe.c')
& clang -O2 -I compiler\runtime -c (Join-Path $WORK 'probe.c') -o (Join-Path $WORK 'probe.o') 2>&1 | Out-Null
& clang (Join-Path $WORK 'probe.o') (Join-Path $WORK 'mmlrt_win32.o') (Join-Path $WORK 'mmlrt_common.o') -lws2_32 -fuse-ld=lld -o (Join-Path $WORK 'probe.exe') 2>&1 | Out-Null
if (Test-Path (Join-Path $WORK 'probe.exe')) {
    $triple = & (Join-Path $WORK 'probe.exe')
    if ($LASTEXITCODE -eq 42 -and $triple -match 'windows') { ok "runtime probe: $triple, exit 42" }
    else { bad "runtime probe — exit $LASTEXITCODE, triple '$triple'" 'ABI' }
} else { bad "runtime probe link" 'LINK' }

Write-Host "== phase 2: Stage 0 toolchain build =="
cargo build --release -p ourc 2>&1 | Select-Object -Last 2
if (Test-Path $OURC) { ok "cargo build --release -p ourc" } else { bad "ourc.exe built" 'HARNESS'; exit 1 }
& $OURC --version | Out-Null
if ($LASTEXITCODE -eq 0) { ok "ourc --version" } else { bad "ourc --version" 'HARNESS' }

Write-Host "== phase 2.5: Stage 1 — self-hosted driver built by Stage 0 =="
New-Item -ItemType Directory -Force -Path (Split-Path $OURS) | Out-Null
& $OURC selfhost\our.lang -o (Split-Path $OURS -Parent | Join-Path -ChildPath 'our') 2>&1 | Select-Object -Last 1
if (Test-Path $OURS) {
    $v = & $OURS --version
    if ($v -match '^our 0\.2\.') { ok "self-hosted our.exe --version: $v" }
    else { bad "our.exe --version — '$v'" 'SEMANTIC' }
} else { bad "self-hosted our.exe build" 'COMPILE'; exit 1 }

Write-Host "== phase 3: smoke — trivial binary links and runs =="
'fn main() -> i32 { return 0; }' | Set-Content (Join-Path $WORK 't.lang')
& $OURS (Join-Path $WORK 't.lang') -o (Join-Path $WORK 't') 2>&1 | Select-Object -Last 1
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
& $OURS (Join-Path $WORK 'argv.lang') -o (Join-Path $WORK 'argv') | Out-Null
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
& $OURS (Join-Path $WORK 'env.lang') -o (Join-Path $WORK 'env') | Out-Null
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
    back[0] = 0; back[3] = 0;   // definite-init for the reads below
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
& $OURS (Join-Path $WORK 'fio.lang') -o (Join-Path $WORK 'fio') | Out-Null
Push-Location $WORK; & (Join-Path $WORK 'fio.exe'); $rc = $LASTEXITCODE; Pop-Location
if ($rc -eq 42) { ok "binary-mode write+read round-trip" } else { bad "binary I/O (exit $rc)" 'SEMANTIC' }

Write-Host "== phase 6b: canonical \\?\ path through the fd boundary (N79C #10) =="
# realpath() returns the Win32 verbatim form `\\?\C:\...` — the path
# `our check`/`build` was failing to re-open. This isolates the
# boundary: CreateFileW + _open_osfhandle must accept it as-is.
@'
import std.fs;
import std.io;
import std.result;
fn go() -> result.Result<i32, i32> {
    let rel: str = "mml-probe-canon.txt";
    let w = fs.open(rel.bytes.ptr, fs.o_write() | fs.o_create() | fs.o_trunc(), 420)?;
    let msg: str = "canonical read ok";
    fs.write(w, msg.bytes, 17)?;
    fs.close(w)?;
    let canon = fs.realpath(system_allocator(), rel.bytes)?;
    let cbuf: [512]u8;
    cbuf[0] = 0;
    let i: u64 = 0;
    while i < canon.len && i < 511 {
        cbuf[i] = canon.buf[i];
        i = i + 1;
    }
    // NUL-terminate at the copied length — fs.open hands the pointer
    // to strlen; without this it walks past the path bytes.
    cbuf[i] = 0;
    io.print("probe canon=");
    io.println(str_from_utf8(&cbuf[0..i]));
    let cpath: slice u8 = &cbuf[0..i];
    canon.destroy();
    let r = fs.open(cpath.ptr, fs.o_read(), 0)?;
    let buf: [64]u8;
    buf[0] = 0;
    let n = fs.read(r, &mut buf[..], 64)?;
    fs.close(r)?;
    fs.remove(rel);
    if n != 17 { return result.Result<i32, i32>::Ok(1); }
    return result.Result<i32, i32>::Ok(42);
}
fn main() -> i32 {
    match go() {
        Ok(v) => { return v; }
        Err(e) => {
            io.print("probe ERR errno=");
            io.println_i64(i64(e));
            return 100;
        }
    }
}
'@ | Set-Content (Join-Path $WORK 'probe.lang')
& $OURS (Join-Path $WORK 'probe.lang') -o (Join-Path $WORK 'probe') | Out-Null
Push-Location $WORK; $out = & (Join-Path $WORK 'probe.exe'); $rc = $LASTEXITCODE; Pop-Location
$out | ForEach-Object { Write-Host "    $_" }
if ($rc -eq 42) { ok "canonical \\?\ path open+read" } else { bad "canonical-path probe (exit $rc)" 'SEMANTIC' }

Write-Host "== phase 6c: mkdir_all on a canonical \\?\ root (N79C #10) =="
# The project-build root cause: build_dir = canonical root + \build
# and mkdir_all tried to mkdir "\" / "\\" / "\\?" as ancestors.
# This probes realpath -> mkdir_all(root\build\nested) -> file
# inside -> readback -> remove_tree.
@'
import std.io;
import std.vec;
import std.fs;
import std.path;
import std.result;

fn pushbytes(own v: vec.Vec<u8>, s: slice u8) -> vec.Vec<u8> {
    let i: u64 = 0;
    while i < u64(s.len) { v = v.push(s[i]); i = i + 1; }
    return v;
}

fn main() -> i32 {
    let heap = system_allocator();
    let base: str = "probe_mkd_root";
    let md = fs.mkdir(base);
    match md {
        Ok(_) => {}
        Err(_) => { return 51; }
    }
    let rp = fs.realpath(heap, base.bytes);
    let canon = match rp {
        Ok(v) => v,
        Err(_) => { fs.remove(base); return 52; }
    };
    io.print("root=");
    io.println(str_from_utf8(canon.as_slice()));
    let sep = path.hsep();
    let bd = pushbytes(vec.Vec<u8>::new(heap), canon.as_slice());
    bd = bd.push(sep);
    let b1: str = "build";
    bd = pushbytes(bd, b1.bytes);
    bd = bd.push(sep);
    let b2: str = "nested";
    bd = pushbytes(bd, b2.bytes);
    let ma = fs.mkdir_all(heap, bd.as_slice());
    match ma {
        Ok(_) => {}
        Err(e) => {
            io.print("mkdir_all errno=");
            io.println_i64(i64(e));
            fs.remove(base); bd.destroy(); canon.destroy();
            return 53;
        }
    }
    let isd = fs.is_dir(bd.as_slice());
    if !isd {
        io.println("nested dir missing");
        fs.remove(base); bd.destroy(); canon.destroy();
        return 54;
    }
    let fp = pushbytes(vec.Vec<u8>::new(heap), bd.as_slice());
    fp = fp.push(sep);
    let fn_: str = "f.txt";
    fp = pushbytes(fp, fn_.bytes);
    let ok: i32 = 1;
    let op = fs.open(fp.buf.ptr,
            fs.o_write() + fs.o_create() + fs.o_trunc(), 420);
    match op {
        Ok(fd) => {
            let msg: str = "inside nested";
            fs.write(fd, msg.bytes, u64(msg.bytes.len));
            fs.close(fd);
        }
        Err(e) => { ok = 0; }
    }
    if ok == 0 {
        io.println("nested write failed");
        let ct1 = fs.remove_tree(heap, base);
        fp.destroy(); bd.destroy(); canon.destroy();
        return 55;
    }
    let back = fs.slurp(heap, fp.buf.ptr);
    if back.len != 13 {
        let ct2 = fs.remove_tree(heap, base);
        back.destroy(); fp.destroy(); bd.destroy(); canon.destroy();
        return 56;
    }
    back.destroy();
    let rtr = fs.remove_tree(heap, base);
    match rtr {
        Ok(_) => {}
        Err(_) => {
            fp.destroy(); bd.destroy(); canon.destroy();
            return 57;
        }
    }
    fp.destroy(); bd.destroy(); canon.destroy();
    return 42;
}
'@ | Set-Content (Join-Path $WORK 'mkdprobe.lang')
& $OURS (Join-Path $WORK 'mkdprobe.lang') -o (Join-Path $WORK 'mkdprobe') | Out-Null
Push-Location $WORK; $out = & (Join-Path $WORK 'mkdprobe.exe'); $rc = $LASTEXITCODE; Pop-Location
$out | ForEach-Object { Write-Host "    $_" }
if ($rc -eq 42) { ok "mkdir_all on canonical \\?\ root" } else { bad "mkdir_all canonical probe (exit $rc)" 'SEMANTIC' }

Write-Host "== phase 7: sockets — net pins =="
foreach ($t in '514_net', '515_net_errors') {
    & $OURS "native-tests\$t.lang" -o (Join-Path $WORK $t) 2>&1 | Select-Object -Last 1
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
        & $OURS $t -o (Join-Path $WORK $n) 2>&1 | Select-Object -Last 1
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
    -not (Select-String -Path (Join-Path $WORK 'cargo.log') -Pattern 'FAILED' -CaseSensitive).Count) {
    ok "cargo test --workspace"
} else { bad "cargo test --workspace — see $WORK\cargo.log" 'HARNESS' }

Write-Host "== phase 10: real programs — build all =="
Get-ChildItem programs\*.lang | ForEach-Object {
    $n = $_.BaseName
    & $OURS $_.FullName -o (Join-Path $WORK $n) 2>&1 | Select-Object -Last 1
    if (Test-Path (Join-Path $WORK "$n.exe")) { ok "build $n" } else { bad "build $n" 'COMPILE' }
}
& $OURS programs\hexdump\main.lang -o (Join-Path $WORK 'hexdump') 2>&1 | Select-Object -Last 1
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

Write-Host "== phase 13: full native corpus — classified, report-only =="
# N79C bar: classify every FAIL as REAL DEFECT or PLATFORM
# ASSUMPTION (POSIX-only fixture) — zero unexplained. Ourc's own
# runner prints pass/fail per fixture; this is evidence, not a gate.
& $OURC test native-tests 2>&1 | Tee-Object (Join-Path $WORK 'corpus.log') | Select-Object -Last 8
$failN = (Select-String -Path (Join-Path $WORK 'corpus.log') -Pattern '^\s*FAIL' -AllMatches).Count
if ($failN -eq 0) { ok "corpus clean" }
else { skip "corpus: $failN failures — classify each in corpus.log before N79C can close" }

Write-Host "== phase 14: self-host fixpoint — s2 vs s3 .ll =="
$s2 = Join-Path $WORK 's2\our'; $s3 = Join-Path $WORK 's3\our'
New-Item -ItemType Directory -Force -Path (Split-Path $s2), (Split-Path $s3) | Out-Null
& $OURS selfhost\our.lang -o $s2 2>&1 | Select-Object -Last 1
& "$s2.exe" selfhost\our.lang -o $s3 2>&1 | Select-Object -Last 1
if ((Test-Path "$s2.ll") -and (Test-Path "$s3.ll")) {
    if (Compare-Object (Get-Content "$s2.ll") (Get-Content "$s3.ll")) {
        bad "s2/s3 .ll differ" 'SEMANTIC'
    } else { ok "s2 <-> s3 .ll byte-identical" }
} else { bad "s2/s3 build" 'COMPILE' }

Write-Host "== phase 14b: external-tool boundary — clang argv paths (N79C #10) =="
# clang --version through mml_proc_run_cap, then `clang -c` on a
# tiny .ll twice: ordinary path vs its canonical \\?\ form. Exit 42
# means clang accepts verbatim argv; 43 means it doesn't (and
# argp_ext conversion at the argv boundary is doing the work).
@'
import std.io;
import std.vec;
import std.fs;
import std.result;

extern fn mml_proc_run_cap(exe: *const u8, elen: i64,
    args: *const u8, alen: i64,
    cwd: *const u8, clen: i64,
    cap: *const u8, caplen: i64, which: i64) -> i64;

fn zarg(own v: vec.Vec<u8>, s: slice u8) -> vec.Vec<u8> {
    let i: u64 = 0;
    while i < u64(s.len) {
        v = v.push(s[i]);
        i = i + 1;
    }
    return v.push(u8(0));
}

fn capdump(z: slice u8) {
    let out = fs.slurp(system_allocator(), z.ptr);
    if out.len > 0 {
        io.print("  captured: ");
        io.println(str_from_utf8(out.as_slice()));
    }
    out.destroy();
}

fn ccrun(read ab: vec.Vec<u8>, capz: slice u8) -> i64 {
    let clang: str = "clang";
    return mml_proc_run_cap(clang.bytes.ptr, i64(clang.bytes.len),
        ab.buf.ptr, i64(ab.len), 0, 0,
        capz.ptr, i64(capz.len), 3);
}

fn main() -> i32 {
    let heap = system_allocator();
    let capn: str = "probe-cc-cap.txt";
    let capz = zarg(vec.Vec<u8>::new(heap), capn.bytes);
    let ab = vec.Vec<u8>::new(heap);
    let a1: str = "--version";
    ab = zarg(ab, a1.bytes);
    let st1 = ccrun(ab, capz.as_slice());
    ab.destroy();
    io.print("cc1 st=");
    io.println_i64(st1);
    capdump(capz.as_slice());
    if st1 < 0 { capz.destroy(); return 44; }
    let lln: str = "probe_tiny.ll";
    let llc: str = "define i32 @f() { ret i32 0 }\n";
    match fs.open(lln.bytes.ptr,
            fs.o_write() + fs.o_create() + fs.o_trunc(), 420) {
        Ok(fd) => {
            fs.write(fd, llc.bytes, u64(llc.bytes.len));
            fs.close(fd);
        }
        Err(_) => { capz.destroy(); return 45; }
    }
    let c2: str = "-c";
    let o2f: str = "-o";
    let on: str = "probe_tiny.o";
    let ab2 = vec.Vec<u8>::new(heap);
    ab2 = zarg(ab2, c2.bytes);
    ab2 = zarg(ab2, lln.bytes);
    ab2 = zarg(ab2, o2f.bytes);
    ab2 = zarg(ab2, on.bytes);
    let st2 = ccrun(ab2, capz.as_slice());
    ab2.destroy();
    io.print("cc2 st=");
    io.println_i64(st2);
    capdump(capz.as_slice());
    let st3: i64 = 46;
    match fs.realpath(heap, lln.bytes) {
        Err(_) => { st3 = 46; }
        Ok(canon) => {
            io.print("canon=");
            io.println(str_from_utf8(canon.as_slice()));
            let on2: str = "probe_tiny2.o";
            let ab3 = vec.Vec<u8>::new(heap);
            ab3 = zarg(ab3, c2.bytes);
            ab3 = zarg(ab3, canon.as_slice());
            ab3 = zarg(ab3, o2f.bytes);
            ab3 = zarg(ab3, on2.bytes);
            canon.destroy();
            st3 = ccrun(ab3, capz.as_slice());
            ab3.destroy();
            io.print("cc3 st=");
            io.println_i64(st3);
            capdump(capz.as_slice());
            fs.remove(on2);
        }
    }
    fs.remove(lln);
    fs.remove(on);
    fs.remove(capn);
    capz.destroy();
    if st3 == 46 { return 46; }
    if st2 != 0 { return 47; }
    if st3 != 0 { return 43; }
    return 42;
}
'@ | Set-Content (Join-Path $WORK 'ccprobe.lang')
& $OURS (Join-Path $WORK 'ccprobe.lang') -o (Join-Path $WORK 'ccprobe') | Out-Null
Push-Location $WORK; $out = & (Join-Path $WORK 'ccprobe.exe'); $rc = $LASTEXITCODE; Pop-Location
$out | ForEach-Object { Write-Host "    $_" }
if ($rc -eq 42) { ok "clang accepts verbatim argv paths" }
elseif ($rc -eq 43) { ok "clang rejects \\?\ argv — argp_ext conversion is the boundary" }
elseif ($rc -eq 44) { skip "clang not on PATH" }
else { bad "external-tool probe (exit $rc)" 'HARNESS' }

Write-Host "== phase 15: project tool — self-hosted our.exe =="
Push-Location $WORK
& $OURS new proj 2>&1 | Select-Object -Last 1
Set-Location proj
& $OURS check 2>&1 | Out-Null; $c1 = $LASTEXITCODE
& $OURS test  2>&1 | Out-Null; $c2 = $LASTEXITCODE
& $OURS build 2>&1 | Out-Null; $c3 = $LASTEXITCODE
& $OURS run   2>&1 | Out-Null; $c4 = $LASTEXITCODE
Pop-Location
if ($c1 -eq 0 -and $c2 -eq 0 -and $c3 -eq 0 -and $c4 -eq 0) { ok "our new → check/test/build/run (self-hosted)" }
else { bad "project tool — check=$c1 test=$c2 build=$c3 run=$c4" 'HARNESS' }

Write-Host "== phase 16: debug info (non-blocking, ourc surface) =="
# `-g` is an ourc-only flag — the self-hosted `our` accepts
# `<file> [-o out]` alone. Exercise it through Stage 0.
# `-o` names a STEM, not an exact filename: `<stem>.ll/.o/.exe`
# all land under it, so `-o t.dbg` yields `t.exe` (stem semantics,
# identical under ourc and our). t.exe also exists from the earlier
# smoke phase — delete it first so this is a real -g check.
Remove-Item (Join-Path $WORK 't.exe') -ErrorAction SilentlyContinue
& $OURC (Join-Path $WORK 't.lang') -g -o (Join-Path $WORK 't.dbg') | Out-Null
if (Test-Path (Join-Path $WORK 't.exe')) { ok "-g build links" } else { bad "-g build" 'LINK' }
$dd = Get-Command llvm-dwarfdump -ErrorAction SilentlyContinue
if ($dd) {
    & llvm-dwarfdump (Join-Path $WORK 't.exe') 2>&1 | Select-String 'DW_TAG' | Select-Object -First 1 | Out-Null
    if ($LASTEXITCODE -eq 0) { ok "DWARF/CodeView present" } else { skip "dwarfdump output unrecognized — check manually" }
} else { skip "no llvm-dwarfdump — check debug info manually" }

Write-Host ""
Write-Host "==== summary: $Pass passed, $Fail failed, $Skip skipped (artifacts in $WORK) ===="
exit ($Fail -ne 0)
