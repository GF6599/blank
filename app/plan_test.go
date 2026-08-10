package app

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

// mustEnumerate fails the test on a refusal, which no case here is about.
func mustEnumerate(t *testing.T, args ...string) Plan {
	t.Helper()
	plan, err := Enumerate(args)
	if err != nil {
		t.Fatalf("enumerating %v: %v", args, err)
	}
	return plan
}

// Enumeration reports what a run would destroy and destroys nothing.
// Given a tree of files, when it is enumerated, then the plan names every entry
// and every file still holds its contents.
// Why it matters: the dry run and the confirmation prompt are both built from
// this list, so an enumeration with a side effect would destroy the tree while
// the operator is still deciding about it.
func TestEnumerateTouchesNothing(t *testing.T) {
	// Given: a tree holding a file, a symlink, and a directory.
	root := t.TempDir()
	kept := filepath.Join(root, "kept.txt")
	if err := os.WriteFile(kept, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the file: %v", err)
	}
	link := filepath.Join(root, "link")
	if err := os.Symlink(kept, link); err != nil {
		t.Fatalf("creating the symlink: %v", err)
	}

	// When: the tree is enumerated.
	plan := mustEnumerate(t, root)

	// Then: the plan names each entry in the bucket that decides its fate.
	if !slices.Equal(plan.Files, []string{kept}) {
		t.Fatalf("the plan lists files %v, expected only %s", plan.Files, kept)
	}
	if !slices.Equal(plan.Removals, []string{link}) {
		t.Fatalf("the plan lists removals %v, expected only %s", plan.Removals, link)
	}
	if !slices.Equal(plan.Directories, []string{root}) {
		t.Fatalf("the plan lists directories %v, expected only %s", plan.Directories, root)
	}

	// And: nothing on disk moved.
	body, err := os.ReadFile(kept)
	if err != nil || string(body) != "payload" {
		t.Fatalf("enumeration destroyed the file: contents are %q, err=%v", body, err)
	}
	if _, err := os.Lstat(link); err != nil {
		t.Fatalf("enumeration removed the symlink: %v", err)
	}
}

// The plan lists directories in the order a run removes them.
// Given a nested tree, when it is enumerated, then the deepest directory is
// listed before the one holding it.
// Why it matters: the plan is what the operator is shown, so listing the tree
// top down would describe an order the run does not use, and a reader could not
// tell from it which directory the run reaches first.
func TestPlanListsTheDeepestDirectoryFirst(t *testing.T) {
	// Given: a tree two directories deep.
	root := t.TempDir()
	deep := filepath.Join(root, "middle", "deep")
	if err := os.MkdirAll(deep, 0o700); err != nil {
		t.Fatalf("creating the tree: %v", err)
	}

	// When: the tree is enumerated and read back in run order.
	var listed []string
	for _, target := range mustEnumerate(t, root).Targets() {
		listed = append(listed, target.Path)
	}

	// Then: each directory appears after the one inside it.
	deepAt := slices.Index(listed, deep)
	rootAt := slices.Index(listed, root)
	if deepAt < 0 || rootAt < 0 {
		t.Fatalf("expected both directories in %v", listed)
	}
	if deepAt > rootAt {
		t.Fatalf("%s is listed at %d, before %s at %d", root, rootAt, deep, deepAt)
	}
}

// A target the enumeration cannot read travels with the plan.
// Given an argument that does not exist, when it is enumerated, then the plan
// reports it and holds no targets.
// Why it matters: the run reports what survived it, and a target it never
// reached has to reach that same list, or a mistyped argument exits zero having
// destroyed nothing.
func TestPlanCarriesTheFailuresOfItsOwnEnumeration(t *testing.T) {
	// Given / When: a path that was never created.
	missing := filepath.Join(t.TempDir(), "does-not-exist")
	plan := mustEnumerate(t, missing)

	// Then: the plan holds nothing to destroy.
	if plan.Total() != 0 {
		t.Fatalf("expected no targets, got %d", plan.Total())
	}
	// And: it reports the argument it could not read.
	err := plan.Err()
	if err == nil {
		t.Fatal("a missing argument enumerated cleanly")
	}
	if !strings.Contains(err.Error(), missing) {
		t.Fatalf("the error does not name the path: %v", err)
	}
}

func TestPlanRecognisesAContainedPathThroughASymlink(t *testing.T) {
	root := t.TempDir()
	alias := filepath.Join(t.TempDir(), "alias")
	if err := os.Symlink(root, alias); err != nil {
		t.Fatalf("creating alias: %v", err)
	}
	plan, err := Enumerate([]string{root})
	if err != nil {
		t.Fatalf("enumerating root: %v", err)
	}
	if !plan.Contains(filepath.Join(alias, "manifest.log")) {
		t.Fatal("the plan did not recognise a log path through a symlink into its tree")
	}
}

func TestPlanRecognisesASelectedFileThroughAHardlink(t *testing.T) {
	selected := filepath.Join(t.TempDir(), "selected")
	if err := os.WriteFile(selected, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing selected file: %v", err)
	}
	alias := filepath.Join(t.TempDir(), "manifest.log")
	if err := os.Link(selected, alias); err != nil {
		t.Fatalf("creating hardlink: %v", err)
	}
	plan, err := Enumerate([]string{selected})
	if err != nil {
		t.Fatalf("enumerating selected file: %v", err)
	}
	if !plan.Contains(alias) {
		t.Fatal("the plan did not recognise a manifest hardlinked to a selected file")
	}
}

// A dry run and a real run describe a target with the same words.
// Given a tree holding one of each kind, when the plan is read, then each target
// carries the verb the activity stream uses for it.
// Why it matters: two sets of wording drift apart, and the operator would
// approve a plan phrased one way and then watch a run phrased another.
func TestPlanTargetsCarryTheActivityVerbs(t *testing.T) {
	// Given: a tree holding a file, a symlink, and the directory itself.
	root := t.TempDir()
	regular := filepath.Join(root, "file.txt")
	if err := os.WriteFile(regular, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the file: %v", err)
	}
	link := filepath.Join(root, "link")
	if err := os.Symlink(regular, link); err != nil {
		t.Fatalf("creating the symlink: %v", err)
	}

	// When: the plan is read as targets.
	verbs := map[string]string{}
	for _, target := range mustEnumerate(t, root).Targets() {
		verbs[target.Path] = target.Verb
	}

	// Then: each one reads as the run will report it.
	for path, want := range map[string]string{regular: verbFile, link: verbRemoval, root: verbDir} {
		if got := verbs[path]; got != want {
			t.Fatalf("%s is planned as %q, the run reports it as %q", path, got, want)
		}
	}
}
