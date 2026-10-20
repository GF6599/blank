package app

import (
	"context"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
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
type writeProgressFunc func(int64)

// math/rand/v2 rather than crypto/rand: since Go 1.22 the top-level functions
// run on a ChaCha8 generator that the operating system seeds, and they are safe
// for concurrent use, which the --jobs workers require. A cover has to leave
// the device holding bytes that are not the plaintext, which needs an
// unpredictable stream and not an auditable one.
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
	_, err := writePassesWithProgress(ctx, f, size, passes, fill, nil)
	return err
}

// The byte count is returned beside the error because it is what separates a
// target the run left alone from one it destroyed and could not remove.
func writePassesWithProgress(ctx context.Context, f *os.File, size int64, passes int, fill fillFunc, progress writeProgressFunc) (int64, error) {
	covered := int64(0)
	if size == 0 || passes < 1 {
		return covered, nil
	}
	buf := make([]byte, min(int64(overwriteChunk), size))
	for pass := range passes {
		if _, err := f.Seek(0, io.SeekStart); err != nil {
			return covered, fmt.Errorf("seek for pass %d: %w", pass+1, err)
		}
		for written := int64(0); written < size; {
			if err := ctx.Err(); err != nil {
				return covered, err
			}
			chunk := buf[:min(int64(len(buf)), size-written)]
			fill(chunk)
			n, err := f.Write(chunk)
			written += int64(n)
			covered += int64(n)
			if progress != nil {
				progress(int64(n))
			}
			if err != nil {
				return covered, fmt.Errorf("write on pass %d: %w", pass+1, err)
			}
		}
		// Without this the passes collapse into one write of the last pattern,
		// because the page cache holds every earlier pass and only the final
		// state ever reaches the device.
		if err := f.Sync(); err != nil {
			return covered, fmt.Errorf("sync on pass %d: %w", pass+1, err)
		}
	}
	return covered, nil
}

const obscureAttempts = 128

// obscureName renames the entry to a meaningless name of the same length, and
// returns the new path. The contents are destroyed by the passes, but the
// directory entry still carries the name until it is unlinked, and a name is
// often as telling as the file.
func obscureName(path string) (string, error) {
	dir, base := filepath.Split(path)
	for range obscureAttempts {
		raw := make([]byte, (len(base)+1)/2)
		randomFill(raw)
		renamed := filepath.Join(dir, hex.EncodeToString(raw)[:len(base)])
		if renamed == path {
			continue
		}
		if err := renameNoReplace(path, renamed); err != nil {
			if errors.Is(err, fs.ErrExist) {
				continue
			}
			return path, fmt.Errorf("rename: %w", err)
		}
		return renamed, nil
	}
	return path, fmt.Errorf("rename: could not find an unused name after %d attempts", obscureAttempts)
}

// openForOverwrite opens path for writing, forcing the mode once if the current
// one refuses. It reports anything that is not a regular file rather than
// opening it, because opening a FIFO for writing waits for a reader that a run
// destroying the filesystem is never going to supply.
func openForOverwrite(path string) (*os.File, int64, error) {
	f, err := openRegularNoFollow(path, os.O_WRONLY, 0, true)
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

// damagedTarget marks a failure that stopped after a cover had already landed.
// The entry is still on the disk and its contents are not, which is the opposite
// of what a caller assumes when a run reports a target it did not destroy.
type damagedTarget struct{ err error }

func (d *damagedTarget) Error() string { return d.err.Error() }
func (d *damagedTarget) Unwrap() error { return d.err }

func markDamage(covered int64, err error) error {
	if covered == 0 {
		return err
	}
	return &damagedTarget{err: err}
}

// overwriteFile destroys the contents of a regular file, then its name, then the
// entry. It replaces `shred -uzfn <passes>`: passes random covers, one zero
// cover, a rename, and an unlink.
//
// A failure before the first cover leaves the file exactly as it was. A failure
// after one leaves it on the disk holding random bytes, and says so, because
// removing a file whose overwrite failed makes plaintext unrecoverable to the
// caller while leaving it recoverable from the disk.
func overwriteFile(ctx context.Context, path string, passes int) error {
	return overwriteFileWithProgress(ctx, path, passes, nil)
}

func overwriteFileWithProgress(ctx context.Context, path string, passes int, progress writeProgressFunc) error {
	if passes < 0 {
		return fmt.Errorf("passes cannot be negative, got %d", passes)
	}
	f, size, err := openForOverwrite(path)
	if err != nil {
		return err
	}
	covered, err := writePassesWithProgress(ctx, f, size, passes, randomFill, progress)
	if err != nil {
		f.Close()
		return markDamage(covered, err)
	}
	zeroed, err := writePassesWithProgress(ctx, f, size, 1, zeroFill, progress)
	covered += zeroed
	if err != nil {
		f.Close()
		return markDamage(covered, err)
	}
	if err := f.Close(); err != nil {
		return markDamage(covered, fmt.Errorf("close: %w", err))
	}
	// Checked here as well because the passes above return early on a zero
	// length file, which would otherwise let a cancelled run still unlink it.
	if err := ctx.Err(); err != nil {
		return markDamage(covered, err)
	}
	renamed, err := obscureName(path)
	if err != nil {
		return markDamage(covered, err)
	}
	if err := os.Remove(renamed); err != nil {
		// The rename already moved the entry, so the name in the report is not
		// the name left on the disk unless this says which one to look for.
		return markDamage(covered, fmt.Errorf("remove %s: %w", displayText(renamed), err))
	}
	return nil
}
