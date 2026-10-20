#!/usr/bin/env python3
"""
gen-dummy-df.py - Create a rich set of filesystem artifacts for testing.

Examples:
  python3 utils/gen-dummy-df.py --root ./test_fs_data --force
  python3 utils/gen-dummy-df.py --root ./tmp/data --large-file-size 20 --many-files 500 --depth 8 --force

Notes:
- Designed for POSIX (Linux/macOS). It will gracefully skip features your OS doesn't support
  (e.g., FIFOs or UNIX sockets on Windows, or symlinks without permissions).
- Use --force to recreate the target directory if it already exists.
- Creates: regular files, hidden files/dirs, symlinks (incl. dangling), hard links, an executable,
  named pipe (FIFO), UNIX socket file, deep trees, long filenames, Unicode names, a sparse file, and more.
"""

import argparse
import os
import shutil
import socket
import stat
import sys
from datetime import datetime, timezone
from pathlib import Path


def log(msg: str) -> None:
    print(f"[+] {msg}")


def create_text_file(path: Path, content: str) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        f.write(content)
    return path


def create_binary_file(path: Path, size_bytes: int) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        # Write in chunks to avoid large memory peaks
        chunk = os.urandom(8192)
        written = 0
        while written < size_bytes:
            to_write = min(len(chunk), size_bytes - written)
            f.write(chunk[:to_write])
            written += to_write
    return path


def make_executable(path: Path) -> None:
    try:
        mode = path.stat().st_mode
        path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    except Exception as e:
        log(f"  ! couldn't mark executable on {path}: {e}")


def make_no_perms(path: Path) -> None:
    try:
        path.chmod(0)
    except Exception as e:
        log(f"  ! couldn't chmod 000 on {path}: {e}")


def make_symlink(target: str | Path, linkpath: Path) -> bool:
    try:
        if linkpath.exists() or linkpath.is_symlink():
            linkpath.unlink()
        os.symlink(target, linkpath)
        return True
    except Exception as e:
        log(f"  ! symlink skipped ({linkpath} -> {target}): {e}")
        return False


def make_hardlink(src: Path, linkpath: Path) -> bool:
    try:
        if linkpath.exists():
            linkpath.unlink()
        os.link(src, linkpath)
        return True
    except Exception as e:
        log(f"  ! hardlink skipped ({linkpath} -> {src}): {e}")
        return False


def make_fifo(path: Path) -> bool:
    try:
        if hasattr(os, "mkfifo"):
            os.mkfifo(path)
            return True
        raise OSError("os.mkfifo not available on this platform")
    except Exception as e:
        log(f"  ! FIFO skipped ({path}): {e}")
        return False


def make_unix_socket(path: Path) -> bool:
    try:
        if hasattr(socket, "AF_UNIX"):
            # Create a filesystem entry of type 'socket'
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                sock.bind(str(path))
            finally:
                sock.close()
            return True
        raise OSError("AF_UNIX not available on this platform")
    except Exception as e:
        log(f"  ! UNIX socket skipped ({path}): {e}")
        return False


def set_times(path: Path, atime: int, mtime: int) -> None:
    try:
        os.utime(path, (atime, mtime), follow_symlinks=False)
    except Exception as e:
        log(f"  ! utime skipped on {path}: {e}")


def unlock_tree(root: Path) -> None:
    """Make every directory readable, so the tree can be removed again.

    The walk is top-down because each directory is chmodded before it is
    descended into, which is the only way it reaches what a mode 000 directory
    holds. rmtree on its own stops at one and leaves the tree behind.
    """
    for parent, dirs, _ in os.walk(root, onerror=lambda e: log(f"  ! walk skipped: {e}")):
        for name in dirs:
            try:
                os.chmod(os.path.join(parent, name), 0o700)
            except OSError as e:
                log(f"  ! chmod skipped ({name}): {e}")


# The same list app/guard.go refuses, because --force removes --root and
# unlock_tree clears the permissions that would otherwise have stopped it.
SYSTEM_PATHS = [
    "/bin", "/boot", "/cores", "/dev", "/etc", "/home", "/lib", "/lib32",
    "/lib64", "/opt", "/proc", "/root", "/run", "/sbin", "/srv", "/sys",
    "/usr", "/var",
    "/Applications", "/Library", "/System", "/Users", "/Volumes", "/private",
]


def refuse_protected(root: Path) -> None:
    """Stop on a root that holds everything else.

    There is no override flag, matching the tool this generates data for. The
    answer to a refusal is to name a narrower directory.
    """
    real = root.resolve()
    if real.parent == real:
        sys.exit(f"[!] {real} is the root of a filesystem. Refusing.")
    protected = [Path(p).resolve() for p in SYSTEM_PATHS] + [Path.home().resolve()]
    for p in protected:
        if real == p:
            sys.exit(f"[!] {real} is a protected path. Refusing.")
        if p.is_relative_to(real):
            sys.exit(f"[!] {real} holds the protected path {p}. Refusing.")


def create_sparse_file(path: Path, size_bytes: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as f:
        if size_bytes <= 0:
            return
        f.seek(size_bytes - 1)
        f.write(b"\0")


def build(root: Path, large_mb: int, many: int, depth: int) -> Path:
    root = root.resolve()
    log(f"Target root: {root}")
    root.mkdir(parents=True, exist_ok=True)

    # Basic directories
    (root / "alpha").mkdir(exist_ok=True)
    (root / "beta" / "subdir").mkdir(parents=True, exist_ok=True)
    (root / ".hidden_dir").mkdir(exist_ok=True)
    (root / "with spaces").mkdir(exist_ok=True)
    (root / "utf8_目录").mkdir(exist_ok=True)

    # Regular & hidden files
    create_text_file(root / "alpha" / "hello.txt", "hello world\n")
    create_text_file(root / ".hidden_file", "shh\n")
    create_text_file(root / "with spaces" / "file with spaces.txt", "spaces!\n")
    create_text_file(root / "utf8_目录" / "文件名.txt", "unicode content\n")

    # Empty file & executable
    (root / "empty").touch()
    script = create_text_file(
        root / "run_me.sh", "#!/usr/bin/env bash\necho 'I am executable'\n"
    )
    make_executable(script)

    # Binary file
    create_binary_file(root / "beta" / "subdir" / "random.bin", 1024)

    # Large file (sparse) and a smaller real file (capped for speed)
    create_sparse_file(root / "sparse_bigfile.bin", large_mb * 1024 * 1024)
    create_binary_file(root / "real_bigfile.bin", min(large_mb, 10) * 1024 * 1024)

    # Hard and soft links
    make_symlink(
        "../alpha/hello.txt", root / "beta" / "hello_symlink.txt"
    )  # relative link
    make_hardlink(root / "alpha" / "hello.txt", root / "alpha" / "hello.hardlink")

    # Symlink to dir & dangling symlink
    make_symlink("beta/subdir", root / "link_to_dir")
    make_symlink("missing_target", root / "dangling_link")

    # Named pipe (FIFO) and UNIX socket
    make_fifo(root / "named_pipe")
    make_unix_socket(root / "unix_socket")

    # Permission edge cases. no_list_dir holds a file, so a run that cannot list
    # the directory cannot empty it either and has to leave it where it is.
    create_text_file(root / "no_read.txt", "no read please\n")
    create_text_file(root / "no_list_dir" / "unreachable.txt", "cannot be reached\n")
    make_no_perms(root / "no_read.txt")
    make_no_perms(root / "no_list_dir")

    # Timestamps (old & future)
    old = int(datetime(2000, 1, 1, 12, 0, 0, tzinfo=timezone.utc).timestamp())
    future = int(datetime(2030, 1, 1, 12, 0, 0, tzinfo=timezone.utc).timestamp())
    set_times(root / "alpha" / "hello.txt", old, old)
    set_times(root / "empty", future, future)

    # Long filename
    long_name = "a" * 200 + ".txt"
    create_text_file(root / long_name, "long name\n")

    # Deep nesting
    current = root
    for i in range(1, depth + 1):
        current = current / f"level_{i}"
        current.mkdir(exist_ok=True)
        create_text_file(current / f"note_{i}.txt", f"This is level {i}\n")

    # Many small files
    mfdir = root / "many_files"
    mfdir.mkdir(exist_ok=True)
    for i in range(1, many + 1):
        create_text_file(mfdir / f"file_{i:04d}.txt", f"{i}\n")

    # Circular symlinks (dir <-> dir)
    (root / "A").mkdir(exist_ok=True)
    (root / "B").mkdir(exist_ok=True)
    make_symlink("../B", root / "A" / "to_B")
    make_symlink("../A", root / "B" / "to_A")

    log("Done.")
    return root


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Scaffold filesystem test data.")
    p.add_argument("--root", default="test_fs_data", help="Target directory to create")
    p.add_argument(
        "--force", action="store_true", help="Delete --root if it already exists"
    )
    p.add_argument(
        "--large-file-size",
        type=int,
        default=5,
        help="Size in MB for big files (sparse=exact, real file capped at 10MB)",
    )
    p.add_argument(
        "--many-files",
        type=int,
        default=100,
        help="Number of small files in many_files/",
    )
    p.add_argument("--depth", type=int, default=5, help="Depth of nested directories")
    return p.parse_args()


def main() -> None:
    args = parse_args()
    root = Path(args.root)
    refuse_protected(root)
    if root.exists():
        if args.force:
            log(f"--force set; removing existing {root}")
            unlock_tree(root)
            shutil.rmtree(root)
        else:
            log(
                f"{root} already exists. Use --force to recreate or choose another --root."
            )
            sys.exit(1)
    root = build(root, args.large_file_size, args.many_files, args.depth)
    log(f"Created test data under: {root}")
    log(f"Tip: when you're done, remove it with: chmod -R u+rwX {root} && rm -rf {root}")


if __name__ == "__main__":
    main()
