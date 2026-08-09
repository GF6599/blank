package app

import (
	"context"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"io"
	"math/rand/v2"
	"os"
	"path/filepath"
)

// overwriteChunk is how much is written per call. Large enough that the syscall
// cost disappears against the write itself, small enough that a cancelled run
// stops without finishing a large file.
const overwriteChunk = 1 << 20

// forcedMode is what a file that refuses to open for writing is set to, which is
// what `shred -f` does. Nothing narrower would let the write through, and the
// entry is unlinked moments later.
const forcedMode os.FileMode = 0o600

type fillFunc func([]byte)

func randomFill(buf []byte) {
	n := len(buf) &^ 7
	for i := 0; i < n; i += 8 {
		binary.LittleEndian.PutUint64(buf[i:], rand.Uint64())
	}
	for i, r := n, rand.Uint64(); i < len(buf); i++ {
		buf[i] = byte(r >> (8 * (i - n)))
	}
}

func zeroFill(buf []byte) { clear(buf) }

// writePasses covers the first size bytes of f, passes times over. The buffer is
// refilled for every chunk, so a random pass never repeats a block pattern that
// would survive as a recognisable stripe.
func writePasses(ctx context.Context, f *os.File, size int64, passes int, fill fillFunc) error {
	if size == 0 || passes < 1 {
		return nil
	}
	buf := make([]byte, min(int64(overwriteChunk), size))
	for pass := range passes {
		if _, err := f.Seek(0, io.SeekStart); err != nil {
			return fmt.Errorf("seek for pass %d: %w", pass+1, err)
		}
		for written := int64(0); written < size; {
			if err := ctx.Err(); err != nil {
				return err
			}
			chunk := buf[:min(int64(len(buf)), size-written)]
			fill(chunk)
			n, err := f.Write(chunk)
			if err != nil {
				return fmt.Errorf("write on pass %d: %w", pass+1, err)
			}
			written += int64(n)
		}
		// Without this the passes collapse into one write of the last pattern,
		// because the page cache holds every earlier pass and only the final
		// state ever reaches the device.
		if err := f.Sync(); err != nil {
			return fmt.Errorf("sync on pass %d: %w", pass+1, err)
		}
	}
	return nil
}

// obscureName renames the entry to a meaningless name of the same length, and
// returns the new path. The contents are destroyed by the passes, but the
// directory entry still carries the name until it is unlinked, and a name is
// often as telling as the file.
func obscureName(path string) (string, error) {
	dir, base := filepath.Split(path)
	raw := make([]byte, (len(base)+1)/2)
	randomFill(raw)
	renamed := filepath.Join(dir, hex.EncodeToString(raw)[:len(base)])
	if err := os.Rename(path, renamed); err != nil {
		return path, fmt.Errorf("rename: %w", err)
	}
	return renamed, nil
}

// openForOverwrite opens path for writing, forcing the mode once if the current
// one refuses. It reports anything that is not a regular file rather than
// opening it, because opening a FIFO for writing waits for a reader that a run
// destroying the filesystem is never going to supply.
func openForOverwrite(path string) (*os.File, int64, error) {
	fi, err := os.Lstat(path)
	if err != nil {
		return nil, 0, fmt.Errorf("read path: %w", err)
	}
	if !fi.Mode().IsRegular() {
		return nil, 0, fmt.Errorf("not a regular file (mode %s)", fi.Mode())
	}
	f, err := os.OpenFile(path, os.O_WRONLY, 0)
	if os.IsPermission(err) {
		if chmodErr := os.Chmod(path, forcedMode); chmodErr != nil {
			return nil, 0, fmt.Errorf("open for writing: %w (chmod also failed: %v)", err, chmodErr)
		}
		f, err = os.OpenFile(path, os.O_WRONLY, 0)
	}
	if err != nil {
		return nil, 0, fmt.Errorf("open for writing: %w", err)
	}
	// Sized from the open handle rather than the Lstat above, so the length
	// written is the length of the file actually opened.
	opened, err := f.Stat()
	if err != nil {
		f.Close()
		return nil, 0, fmt.Errorf("stat the open file: %w", err)
	}
	if !opened.Mode().IsRegular() {
		f.Close()
		return nil, 0, fmt.Errorf("not a regular file (mode %s)", opened.Mode())
	}
	return f, opened.Size(), nil
}

// overwriteFile destroys the contents of a regular file, then its name, then the
// entry. It replaces `shred -uzfn <passes>`: passes random covers, one zero
// cover, a rename, and an unlink.
//
// The file is left in place when anything fails, so the caller can report it as
// surviving. Removing a file whose overwrite failed is what makes plaintext
// unrecoverable to the caller while leaving it recoverable from the disk.
func overwriteFile(ctx context.Context, path string, passes int) error {
	if passes < 0 {
		return fmt.Errorf("passes cannot be negative, got %d", passes)
	}
	f, size, err := openForOverwrite(path)
	if err != nil {
		return err
	}
	if err := writePasses(ctx, f, size, passes, randomFill); err != nil {
		f.Close()
		return err
	}
	if err := writePasses(ctx, f, size, 1, zeroFill); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return fmt.Errorf("close: %w", err)
	}
	// Checked here as well because the passes above return early on a zero
	// length file, which would otherwise let a cancelled run still unlink it.
	if err := ctx.Err(); err != nil {
		return err
	}
	renamed, err := obscureName(path)
	if err != nil {
		return err
	}
	if err := os.Remove(renamed); err != nil {
		return fmt.Errorf("remove: %w", err)
	}
	return nil
}
