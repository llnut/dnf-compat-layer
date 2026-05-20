//! LD_PRELOAD test for the `/dev/shm/<a>/<b>` flattening. Builds the 32-bit
//! x86 `cdylib`, preloads it into a matching C test program, and checks a nested
//! path becomes one object. `#[ignore]` by default since it needs a 32-bit
//! x86 toolchain. Run with:
//!
//! ```sh
//! cargo test -p glibc-compat --test preload_shm -- --ignored --nocapture
//! ```

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const TARGET: &str = "i686-unknown-linux-gnu";

/// Target for `cargo zigbuild`, glibc pinned. The rustc target stays
/// `TARGET`, so the artifact path is the same.
const ZIG_TARGET: &str = "i686-unknown-linux-gnu.2.17";

/// Print a skip notice and return; a missing toolchain is not a failure.
macro_rules! skip {
    ($($arg:tt)*) => {{
        eprintln!("SKIP preload_shm: {}", format!($($arg)*));
        return;
    }};
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("glibc-compat has a parent workspace dir")
        .to_path_buf()
}

fn run(cmd: &mut Command) -> Option<std::process::Output> {
    cmd.output().ok()
}

/// Build the 32-bit x86 cdylib in its own target dir to avoid clashing with
/// the outer build. Prefers `cargo zigbuild`, falls back to `cargo build`.
fn build_cdylib(out_dir: &Path) -> Option<PathBuf> {
    let root = workspace_root();
    let so = out_dir
        .join(TARGET)
        .join("release")
        .join("libglibc_compat.so");

    let attempts: [&[&str]; 2] = [
        &[
            "zigbuild",
            "-p",
            "glibc-compat",
            "--release",
            "--target",
            ZIG_TARGET,
        ],
        &[
            "build",
            "-p",
            "glibc-compat",
            "--release",
            "--target",
            TARGET,
        ],
    ];
    for args in attempts {
        let _ = std::fs::remove_file(&so);
        let out = match run(Command::new(env!("CARGO"))
            .current_dir(&root)
            .args(args)
            .env("CARGO_TARGET_DIR", out_dir))
        {
            Some(o) => o,
            None => continue,
        };
        if out.status.success() && so.exists() {
            return Some(so);
        }
        eprintln!(
            "`cargo {}` did not produce the cdylib:\n{}",
            args[0],
            String::from_utf8_lossy(&out.stderr)
        );
    }
    None
}

const TEST_PROG_C: &str = r#"
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

/* argv[1] is a unique /dev/shm prefix without a trailing slash. */
int main(int argc, char **argv) {
    if (argc < 2) return 2;
    char dir[512], nested[512], renamed[512];
    snprintf(dir, sizeof dir, "%s", argv[1]);
    snprintf(nested, sizeof nested, "%s/leaf", argv[1]);
    snprintf(renamed, sizeof renamed, "%s/leaf2", argv[1]);

    /* mkdir on a /dev/shm target must report success without a real dir. */
    if (mkdir(dir, 0700) != 0) { perror("mkdir"); return 3; }

    int fd = open(nested, O_CREAT | O_RDWR, 0600);
    if (fd < 0) { perror("open create"); return 4; }
    if (write(fd, "ping", 4) != 4) { perror("write"); return 5; }
    close(fd);

    /* rename exercises the two-path hook; both endpoints are /dev/shm. */
    if (rename(nested, renamed) != 0) { perror("rename"); return 6; }

    int fd2 = open(renamed, O_RDONLY);
    if (fd2 < 0) { perror("open read"); return 7; }
    char buf[4] = {0};
    if (read(fd2, buf, 4) != 4 || memcmp(buf, "ping", 4) != 0) {
        close(fd2);
        return 8;
    }
    close(fd2);

    if (unlink(renamed) != 0) { perror("unlink"); return 9; }
    return 0;
}
"#;

/// Compile the 32-bit x86 C test program. Prefers `zig cc`, which needs no
/// gcc-multilib, and falls back to `$CC -m32` then `cc -m32`.
fn compile_test_prog(dir: &Path) -> Option<PathBuf> {
    let src = dir.join("test_prog.c");
    if fs::write(&src, TEST_PROG_C).is_err() {
        return None;
    }
    let bin = dir.join("test_prog");

    let zig_args: Vec<&OsStr> = vec![
        OsStr::new("cc"),
        OsStr::new("-target"),
        OsStr::new("x86-linux-gnu.2.17"),
        OsStr::new("-O0"),
        OsStr::new("-o"),
        bin.as_os_str(),
        src.as_os_str(),
    ];
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let cc_args: Vec<&OsStr> = vec![
        OsStr::new("-m32"),
        OsStr::new("-O0"),
        OsStr::new("-o"),
        bin.as_os_str(),
        src.as_os_str(),
    ];
    let attempts: [(&str, &[&OsStr]); 2] = [("zig", &zig_args), (&cc, &cc_args)];

    for (prog, args) in attempts {
        let _ = fs::remove_file(&bin);
        let out = match run(Command::new(prog).args(args)) {
            Some(o) => o,
            None => continue,
        };
        if out.status.success() && bin.exists() {
            return Some(bin.clone());
        }
        eprintln!(
            "{prog} could not build the 32-bit x86 test_prog:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    None
}

fn unique_shm_prefix() -> String {
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("/dev/shm/dnf_compat_test_{}_{}", std::process::id(), ns)
}

#[test]
#[ignore = "requires a 32-bit x86 build and runtime toolchain; run with --ignored"]
fn dev_shm_nested_path_is_flattened_under_preload() {
    let tmp = std::env::temp_dir().join(format!("dnf_compat_preload_{}", std::process::id()));
    let _ = fs::create_dir_all(&tmp);

    let so = match build_cdylib(&tmp) {
        Some(p) => p,
        None => skip!("could not build the {TARGET} cdylib in this environment"),
    };
    let test_prog = match compile_test_prog(&tmp) {
        Some(p) => p,
        None => skip!("no working 32-bit x86 C toolchain (cc -m32)"),
    };

    let prefix = unique_shm_prefix();
    let flattened = PathBuf::from(format!("{prefix}_leaf2"));
    let nested_dir = PathBuf::from(&prefix);
    let nested_file = PathBuf::from(format!("{prefix}/leaf"));

    let status = Command::new(&test_prog)
        .arg(&prefix)
        .env("LD_PRELOAD", &so)
        .status();

    let cleanup = || {
        let _ = fs::remove_file(&flattened);
        let _ = fs::remove_file(format!("{prefix}_leaf"));
        let _ = fs::remove_file(&nested_file);
        let _ = fs::remove_dir(&nested_dir);
        let _ = fs::remove_dir_all(&tmp);
    };

    let status = match status {
        Ok(s) => s,
        Err(e) => {
            cleanup();
            skip!("cannot execute the 32-bit x86 test_prog ({e})");
        }
    };

    if !status.success() {
        cleanup();
        panic!("test_prog exited with {status}; the flatten invariant is broken");
    }

    let flattened_exists = flattened.exists();
    let nested_dir_exists = nested_dir.exists();
    let nested_file_exists = nested_file.exists();
    cleanup();

    // The unlink hook rewrites the trailing unlink in the test program to
    // the flattened path, so the flattened object is absent after the run.
    assert!(
        !flattened_exists,
        "expected flattened object {flattened:?} to be unlinked by the preload hook"
    );
    assert!(
        !nested_dir_exists,
        "no real directory should be created under /dev/shm"
    );
    assert!(
        !nested_file_exists,
        "the nested path must not resolve to a separate inode"
    );
}
