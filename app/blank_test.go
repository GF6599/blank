package app

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"blank/internal/testsupport"
)

func TestBlankEmitsProgressAndActivity(t *testing.T) {
	logFile := testsupport.SetupStubCommands(t)

	root := t.TempDir()
	file1 := filepath.Join(root, "alpha.txt")
	file2 := filepath.Join(root, "nested", "beta.txt")
	if err := os.MkdirAll(filepath.Dir(file2), 0o755); err != nil {
		t.Fatalf("creating nested dir: %v", err)
	}
	if err := os.WriteFile(file1, []byte("secret-1"), 0o600); err != nil {
		t.Fatalf("writing file1: %v", err)
	}
	if err := os.WriteFile(file2, []byte("secret-2"), 0o600); err != nil {
		t.Fatalf("writing file2: %v", err)
	}

	symlink := filepath.Join(root, "link-to-alpha")
	if err := os.Symlink(file1, symlink); err != nil {
		t.Fatalf("creating symlink: %v", err)
	}

	progressCh := make(chan float64)
	activityCh := make(chan string)

	var progressVals []float64
	var activityVals []string
	var wg sync.WaitGroup
	wg.Add(2)

	go func() {
		defer wg.Done()
		for p := range progressCh {
			progressVals = append(progressVals, p)
		}
	}()

	go func() {
		defer wg.Done()
		for msg := range activityCh {
			activityVals = append(activityVals, msg)
		}
	}()

	Blank(context.Background(), []string{root}, progressCh, activityCh)
	wg.Wait()

	if len(progressVals) == 0 || progressVals[len(progressVals)-1] != 1 {
		t.Fatalf("expected final progress of 1, got %v", progressVals)
	}
	if len(activityVals) == 0 {
		t.Fatalf("expected activity messages, got none")
	}

	if _, err := os.Stat(file1); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", file1, err)
	}
	if _, err := os.Stat(file2); !os.IsNotExist(err) {
		t.Fatalf("expected %s to be removed, err=%v", file2, err)
	}
	if _, err := os.Lstat(symlink); !os.IsNotExist(err) {
		t.Fatalf("expected symlink to be removed, err=%v", err)
	}

	data, err := os.ReadFile(logFile)
	if err != nil {
		t.Fatalf("reading stub log: %v", err)
	}
	if !strings.Contains(string(data), "shred "+file1) {
		t.Fatalf("expected shred log for %s, got %s", file1, string(data))
	}
}

func TestBlankNoTargetsClosesChannels(t *testing.T) {
	progressCh := make(chan float64)
	activityCh := make(chan string)

	doneProgress := make(chan struct{})
	go func() {
		for range progressCh {
			t.Error("unexpected progress message")
		}
		close(doneProgress)
	}()

	doneActivity := make(chan struct{})
	go func() {
		for range activityCh {
			t.Error("unexpected activity message")
		}
		close(doneActivity)
	}()

	missing := filepath.Join(t.TempDir(), "does-not-exist")
	Blank(context.Background(), []string{missing}, progressCh, activityCh)

	<-doneProgress
	<-doneActivity
}
