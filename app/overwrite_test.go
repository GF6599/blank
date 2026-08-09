package app

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"syscall"
	"testing"
)

// witness hardlinks path, so a test can read the inode after the run unlinks
// the name. Overwriting destroys the contents in place, and the contents are
// what the assertions are about.
func witness(t *testing.T, path string) string {
	t.Helper()
	link := path + ".witness"
	if err := os.Link(path, link); err != nil {
		t.Fatalf("hardlinking %s: %v", path, err)
	}
	return link
}

func writeFile(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatalf("writing %s: %v", path, err)
	}
}

// The overwrite replaces every byte the file held.
// Given a file of known contents, when the random passes finish, then no byte of
// the original survives anywhere in the file.
// Why it matters: this is the only promise the tool makes. A pass that seeks
// wrongly, stops at the first chunk, or writes nothing leaves the plaintext on
// disk while every other part of the run reports success.
func TestRandomPassesReplaceEveryByte(t *testing.T) {
	// Given: a file whose contents are easy to find again.
	root := t.TempDir()
	path := filepath.Join(root, "secret.txt")
	original := bytes.Repeat([]byte("SENSITIVE"), 4096)
	writeFile(t, path, original)
	seen := witness(t, path)

	// When: the random passes run.
	f, err := os.OpenFile(path, os.O_WRONLY, 0)
	if err != nil {
		t.Fatalf("opening %s: %v", path, err)
	}
	defer f.Close()
	if err := writePasses(context.Background(), f, int64(len(original)), 2, randomFill); err != nil {
		t.Fatalf("random passes: %v", err)
	}

	// Then: the file is still its original length.
	got, err := os.ReadFile(seen)
	if err != nil {
		t.Fatalf("reading the witness: %v", err)
	}
	if len(got) != len(original) {
		t.Fatalf("length changed from %d to %d", len(original), len(got))
	}
	// And: none of the original survives.
	if bytes.Contains(got, []byte("SENSITIVE")) {
		t.Fatal("the original contents are still on disk after the overwrite")
	}
	// And: the passes wrote a pattern rather than leaving the file untouched.
	if bytes.Equal(got, original) {
		t.Fatal("the file is unchanged")
	}
	// And: the pattern is random rather than a constant, so recovering one
	// block of it says nothing about any other.
	var present [256]bool
	for _, b := range got {
		present[b] = true
	}
	distinct := 0
	for _, ok := range present {
		if ok {
			distinct++
		}
	}
	if distinct < 200 {
		t.Fatalf("the pass wrote %d distinct byte values over %d bytes, expected close to 256",
			distinct, len(got))
	}
}

// The chunk loop stops the moment the context is cancelled.
// Given a file of several chunks and a context already cancelled, when the
// passes run, then they report the cancellation having written nothing.
// Why it matters: the guard after the passes catches a cancelled run either
// way, so only a check inside the loop stops a large file from being written in
// full after the operator has already quit.
func TestWritePassesStopsInsideTheChunkLoop(t *testing.T) {
	// Given: a file of four chunks.
	root := t.TempDir()
	path := filepath.Join(root, "big.bin")
	original := bytes.Repeat([]byte{0xAB}, overwriteChunk*4)
	writeFile(t, path, original)
	seen := witness(t, path)
	f, err := os.OpenFile(path, os.O_WRONLY, 0)
	if err != nil {
		t.Fatalf("opening %s: %v", path, err)
	}
	defer f.Close()

	// And: a context that is already cancelled.
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	// When: the passes run.
	if err := writePasses(ctx, f, int64(len(original)), 10, randomFill); err == nil {
		t.Fatal("the passes ignored the cancellation")
	}

	// Then: not a byte was written.
	got, err := os.ReadFile(seen)
	if err != nil {
		t.Fatalf("reading the witness: %v", err)
	}
	if !bytes.Equal(got, original) {
		t.Fatal("a cancelled pass still wrote to the file")
	}
}

// The final pass leaves zeros behind.
// Given a file that has been overwritten, when the run finishes, then the inode
// holds nothing but zero bytes.
// Why it matters: a file left holding random data advertises that something was
// destroyed there. Zeros are what an unused region of the disk looks like.
func TestOverwriteLeavesZerosBehind(t *testing.T) {
	// Given: a file of known contents.
	root := t.TempDir()
	path := filepath.Join(root, "secret.txt")
	original := bytes.Repeat([]byte("SENSITIVE"), 1024)
	writeFile(t, path, original)
	seen := witness(t, path)

	// When: the overwrite finishes.
	if err := overwriteFile(context.Background(), path, 2); err != nil {
		t.Fatalf("overwrite: %v", err)
	}

	// Then: the inode holds zeros and nothing else.
	got, err := os.ReadFile(seen)
	if err != nil {
		t.Fatalf("reading the witness: %v", err)
	}
	if len(got) != len(original) {
		t.Fatalf("length changed from %d to %d", len(original), len(got))
	}
	if i := bytes.IndexFunc(got, func(r rune) bool { return r != 0 }); i >= 0 {
		t.Fatalf("byte %d is %#x, expected the final pass to zero the file", i, got[i])
	}
	// And: the name is gone.
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", path, err)
	}
}

// A file larger than one chunk is covered end to end.
// Given a file several times the write buffer, when the overwrite finishes, then
// every byte including the last is zero.
// Why it matters: the chunk loop is where an off-by-one leaves the tail of a
// large file untouched, and the tail is as recoverable as the head.
func TestOverwriteCoversAFileLargerThanTheBuffer(t *testing.T) {
	// Given: a file of three and a half chunks, so the last write is partial.
	root := t.TempDir()
	path := filepath.Join(root, "big.bin")
	size := overwriteChunk*3 + overwriteChunk/2
	writeFile(t, path, bytes.Repeat([]byte{0xAB}, size))
	seen := witness(t, path)

	// When: the overwrite finishes.
	if err := overwriteFile(context.Background(), path, 1); err != nil {
		t.Fatalf("overwrite: %v", err)
	}

	// Then: every byte is zero, the last one included.
	got, err := os.ReadFile(seen)
	if err != nil {
		t.Fatalf("reading the witness: %v", err)
	}
	if len(got) != size {
		t.Fatalf("length changed from %d to %d", size, len(got))
	}
	if i := bytes.IndexByte(got, 0xAB); i >= 0 {
		t.Fatalf("byte %d still holds the original value, %d bytes from the end", i, size-i)
	}
}

// An empty file is destroyed rather than refused.
// Given a file of no bytes, when the overwrite finishes, then it reports success
// and the name is gone.
// Why it matters: a zero length file makes the chunk loop run no iterations, and
// a guard written for the normal case can turn that into an error that fails the
// whole run.
func TestOverwriteHandlesAnEmptyFile(t *testing.T) {
	// Given: an empty file.
	root := t.TempDir()
	path := filepath.Join(root, "empty.txt")
	writeFile(t, path, nil)

	// When: the overwrite finishes.
	if err := overwriteFile(context.Background(), path, 3); err != nil {
		t.Fatalf("an empty file was refused: %v", err)
	}

	// Then: the name is gone.
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", path, err)
	}
}

// A file the caller cannot write is made writable and destroyed.
// Given a read-only file, when the overwrite runs, then it succeeds.
// Why it matters: this is what `shred -f` does, and a read-only file is the
// common case rather than an exotic one. Refusing it would leave the plaintext
// and report a failure the caller can do nothing about.
func TestOverwriteRecoversFromAReadOnlyFile(t *testing.T) {
	// Given: a file with no write permission.
	root := t.TempDir()
	path := filepath.Join(root, "readonly.txt")
	writeFile(t, path, []byte("payload"))
	if err := os.Chmod(path, 0o400); err != nil {
		t.Fatalf("making %s read-only: %v", path, err)
	}

	// When: the overwrite runs.
	if err := overwriteFile(context.Background(), path, 1); err != nil {
		t.Fatalf("a read-only file was refused: %v", err)
	}

	// Then: the name is gone.
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", path, err)
	}
}

// A file that is not a regular file is refused, not opened.
// Given a FIFO, when the overwrite is pointed at it, then it returns an error
// straight away.
// Why it matters: opening a FIFO for writing blocks until a reader arrives, so
// an overwrite that opens first and asks questions later stops the run for ever.
func TestOverwriteRefusesANonRegularFile(t *testing.T) {
	// Given: a FIFO with nothing at the other end.
	root := t.TempDir()
	path := filepath.Join(root, "pipe")
	if err := syscall.Mkfifo(path, 0o600); err != nil {
		t.Fatalf("creating the FIFO: %v", err)
	}

	// When: the overwrite is pointed at it.
	err := overwriteFile(context.Background(), path, 1)

	// Then: it refuses rather than blocking.
	if err == nil {
		t.Fatal("the overwrite accepted a FIFO")
	}
	// And: the FIFO is left for the caller to remove.
	if _, err := os.Lstat(path); err != nil {
		t.Fatalf("the overwrite removed a file it refused: %v", err)
	}
}

// A cancelled run stops rather than finishing the file.
// Given a context already cancelled, when the overwrite runs, then it reports
// the cancellation and leaves the name in place.
// Why it matters: quitting has to stop the work in flight. An overwrite that
// ignores cancellation keeps writing after the interface has gone.
func TestOverwriteStopsOnACancelledContext(t *testing.T) {
	// Given: a file and a context that is already cancelled.
	root := t.TempDir()
	path := filepath.Join(root, "secret.txt")
	writeFile(t, path, bytes.Repeat([]byte("payload"), overwriteChunk))
	ctx, cancel := context.WithCancel(context.Background())
	cancel()

	// When: the overwrite runs.
	err := overwriteFile(ctx, path, 10)

	// Then: it reports the cancellation.
	if err == nil {
		t.Fatal("a cancelled overwrite reported success")
	}
	// And: it did not remove the file, so the run can report it as surviving.
	if _, err := os.Lstat(path); err != nil {
		t.Fatalf("a cancelled overwrite still removed the file: %v", err)
	}
}

// The name is replaced before the entry is unlinked.
// Given a file, when its name is obscured, then the old name is gone and a
// different name of the same length holds the same inode.
// Why it matters: a directory entry keeps the name after the contents are
// destroyed, and a filename is often as telling as the file.
func TestObscureNameReplacesTheDirectoryEntry(t *testing.T) {
	// Given: a file with a telling name.
	root := t.TempDir()
	path := filepath.Join(root, "resignation-letter.txt")
	writeFile(t, path, []byte("payload"))
	before, err := os.Stat(path)
	if err != nil {
		t.Fatalf("stat: %v", err)
	}

	// When: the name is obscured.
	renamed, err := obscureName(path)
	if err != nil {
		t.Fatalf("obscuring the name: %v", err)
	}

	// Then: the telling name is gone.
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		t.Fatalf("the original name survives, err=%v", err)
	}
	// And: the new name is a different one of the same length in the same
	// directory, so the entry gives nothing away and still fits.
	if renamed == path {
		t.Fatal("the name did not change")
	}
	if filepath.Dir(renamed) != root {
		t.Fatalf("the file moved to %s, expected it to stay in %s", filepath.Dir(renamed), root)
	}
	if len(filepath.Base(renamed)) != len(filepath.Base(path)) {
		t.Fatalf("the new name is %d characters, the old one was %d",
			len(filepath.Base(renamed)), len(filepath.Base(path)))
	}
	// And: it is the same file.
	after, err := os.Stat(renamed)
	if err != nil {
		t.Fatalf("stat on the new name: %v", err)
	}
	if !os.SameFile(before, after) {
		t.Fatal("the new name points at a different file")
	}
}

// A pass count below zero is refused rather than silently ignored.
// Given a negative pass count, when the overwrite runs, then it returns an error
// and leaves the file alone.
// Why it matters: the count reaches this from a command line flag, and a
// negative one that quietly means "no random passes" would destroy a file with
// less effort than the caller asked for while reporting success.
func TestOverwriteRefusesANegativePassCount(t *testing.T) {
	// Given: a file.
	root := t.TempDir()
	path := filepath.Join(root, "secret.txt")
	writeFile(t, path, []byte("payload"))

	// When: the overwrite is asked for a negative number of passes.
	err := overwriteFile(context.Background(), path, -1)

	// Then: it refuses.
	if err == nil {
		t.Fatal("a negative pass count was accepted")
	}
	// And: the file is untouched.
	if _, err := os.Lstat(path); err != nil {
		t.Fatalf("a refused overwrite still removed the file: %v", err)
	}
}
