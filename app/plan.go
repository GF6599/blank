package app

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
)

// Plan is everything a run would destroy, in one bucket per way of destroying a
// target. A run acts on nothing else, so what the operator is shown before it
// starts is what it works from.
type Plan struct {
	// roots are the absolute arguments the operator selected. They are retained
	// for confirmation and for checking run-owned files against the blast radius.
	roots       []string
	Files       []string
	Removals    []string
	Directories []string
	// Failures names the targets the enumeration itself could not read. They
	// travel with the plan because the run has to report them beside the ones
	// it could not destroy.
	Failures []Failure
}

// Roots returns an independent copy of the selected absolute arguments in
// stable order.
func (p Plan) Roots() []string {
	return slices.Clone(p.roots)
}

// DisplayRoots returns the selected arguments escaped for a terminal.
func (p Plan) DisplayRoots() []string {
	roots := p.Roots()
	for i := range roots {
		roots[i] = displayText(roots[i])
	}
	return roots
}

// Contains reports whether path is one of the selected roots or lies inside a
// directory the plan will remove. Parent symlinks are resolved on both sides so
// two names for the same directory cannot hide a run-owned file inside it.
func (p Plan) Contains(path string) bool {
	want, err := filepath.Abs(path)
	if err != nil {
		return false
	}
	want = canonicalParent(want)
	wantInfo, _ := os.Lstat(want)
	for _, target := range append(slices.Clone(p.roots), append(slices.Clone(p.Files), p.Removals...)...) {
		if canonicalParent(target) == want {
			return true
		}
	}
	if wantInfo != nil && wantInfo.Mode().IsRegular() {
		for _, target := range p.Files {
			if info, err := os.Lstat(target); err == nil && os.SameFile(wantInfo, info) {
				return true
			}
		}
	}
	for _, dir := range p.Directories {
		dir = canonicalParent(dir)
		if dir == want || holds(dir, want) {
			return true
		}
	}
	return false
}

func canonicalParent(path string) string {
	dir, base := filepath.Split(path)
	if resolved, err := filepath.EvalSymlinks(filepath.Clean(dir)); err == nil {
		return filepath.Join(resolved, base)
	}
	return filepath.Clean(path)
}

func (p Plan) Total() int {
	return len(p.Files) + len(p.Removals) + len(p.Directories)
}

// Err reports what the enumeration could not read, in the shape a run reports
// what it could not destroy.
func (p Plan) Err() error {
	if len(p.Failures) == 0 {
		return nil
	}
	return &IncompleteRun{Failures: slices.Clone(p.Failures)}
}

// Target is one entry a run would destroy.
type Target struct {
	Verb string
	Path string
}

// Targets lists every target in the order the run reaches it, worded the way
// the activity stream words it, so a dry run and a real run cannot describe one
// target differently. The paths are already safe to write to a terminal.
func (p Plan) Targets() []Target {
	out := make([]Target, 0, p.Total())
	for _, f := range p.Files {
		out = append(out, Target{Verb: verbFile, Path: displayText(f)})
	}
	for _, r := range p.Removals {
		out = append(out, Target{Verb: verbRemoval, Path: displayText(r)})
	}
	for _, d := range slices.Backward(p.Directories) {
		out = append(out, Target{Verb: verbDir, Path: displayText(d)})
	}
	return out
}

// Enumerate expands the arguments into everything a run would destroy. It reads
// the filesystem and changes nothing, which is what lets the caller report the
// plan, or ask about it, before any of it happens.
func Enumerate(args []string) (Plan, error) {
	e := &enumeration{fails: &failures{}, seen: map[string]bool{}}
	// Every argument is guarded before any of them is expanded, so a command
	// line holding one protected path plans nothing at all.
	guard := NewGuard()
	targets := make([]string, 0, len(args))
	for _, arg := range args {
		absolutePath, err := filepath.Abs(arg)
		if err != nil {
			e.fails.add(arg, fmt.Errorf("resolve path: %w", err))
			continue
		}
		if err := guard.Refuses(absolutePath); err != nil {
			return Plan{}, err
		}
		if !slices.Contains(e.plan.roots, absolutePath) {
			e.plan.roots = append(e.plan.roots, absolutePath)
		}
		targets = append(targets, absolutePath)
	}
	for _, target := range targets {
		k, err := classify(target)
		if err != nil {
			e.fails.add(target, fmt.Errorf("read path: %w", err))
			continue
		}
		e.add(k, target)
	}
	e.plan.Failures = e.fails.list
	return e.plan, nil
}

// Arguments overlap whenever a glob expands beside the directory holding it.
// The second copy of a file is overwritten after the first has unlinked it,
// which reports a survivor over a file that is gone, so seen keeps one target
// out of the plan twice.
type enumeration struct {
	plan  Plan
	fails *failures
	seen  map[string]bool
}

func (e *enumeration) add(k kind, path string) {
	if e.seen[path] {
		return
	}
	e.seen[path] = true
	switch k {
	case kindDir:
		e.walk(path)
	case kindSymlink, kindSpecial:
		e.plan.Removals = append(e.plan.Removals, path)
	default:
		e.plan.Files = append(e.plan.Files, path)
	}
}

// walk collects the tree under dir. A directory it cannot read is recorded
// rather than skipped, because os.ReadDir failing looks exactly like an empty
// directory and the files inside would otherwise never be queued.
func (e *enumeration) walk(dir string) {
	e.plan.Directories = append(e.plan.Directories, dir)
	entries, err := readDir(dir)
	if err != nil {
		e.fails.add(dir, fmt.Errorf("read directory: %w", err))
		return
	}
	for _, entry := range entries {
		fp := filepath.Join(dir, entry.Name())
		k, err := classify(fp)
		if err != nil {
			e.fails.add(fp, fmt.Errorf("read path: %w", err))
			continue
		}
		e.add(k, fp)
	}
}

type kind int

const (
	kindFile kind = iota
	kindDir
	kindSymlink
	// kindSpecial is a FIFO, a socket, or a device node. The overwrite opens
	// its target, and opening one of these blocks until a peer arrives.
	kindSpecial
)

// classify reads the path once. The walk needs both answers for every entry,
// and a single Lstat carries them.
func classify(path string) (kind, error) {
	fi, err := os.Lstat(path)
	if err != nil {
		return kindFile, err
	}
	switch {
	case fi.Mode()&os.ModeSymlink != 0:
		return kindSymlink, nil
	case fi.IsDir():
		return kindDir, nil
	case fi.Mode().IsRegular():
		return kindFile, nil
	default:
		return kindSpecial, nil
	}
}
