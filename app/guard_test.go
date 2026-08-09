package app

import (
	"os"
	"path/filepath"
	"slices"
	"testing"
)

// A run refuses the paths that hold everything else.
// Given the filesystem root, a system directory, the home directory, or a
// directory holding home, when one is named as a target, then the run refuses it
// and plans nothing.
// Why it matters: blank has no undo, so a mistyped argument that expands to the
// whole machine is the one mistake nothing later in the run can soften.
func TestEnumerateRefusesAProtectedPath(t *testing.T) {
	// Given: a home directory the test owns, so nothing here reads the real one.
	home := t.TempDir()
	t.Setenv("HOME", home)

	for name, path := range map[string]string{
		"the filesystem root":      "/",
		"a system directory":       "/etc",
		"the home directory":       home,
		"a directory holding home": filepath.Dir(home),
	} {
		t.Run(name, func(t *testing.T) {
			// When: it is named as the target.
			plan, err := Enumerate([]string{path})

			// Then: the run refuses it, naming the path.
			if err == nil {
				t.Fatalf("%s was accepted as a target", path)
			}
			// And: it planned nothing, so a caller that ignores the error still
			// destroys nothing.
			if plan.Total() != 0 {
				t.Fatalf("a refused run planned %d targets", plan.Total())
			}
		})
	}
}

// One refused argument stops every other argument beside it.
// Given an ordinary tree named alongside the filesystem root, when they are
// enumerated, then the run refuses and the tree is not planned either.
// Why it matters: a command line that names a protected path is a mistake, so
// the arguments beside it are not to be trusted as deliberate. Destroying those
// and refusing only the protected one would act on half a mistyped command.
func TestARefusedArgumentStopsEveryArgumentBesideIt(t *testing.T) {
	// Given: an ordinary tree with something to lose.
	root := t.TempDir()
	if err := os.WriteFile(filepath.Join(root, "victim.txt"), []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the victim: %v", err)
	}

	// When: it is named alongside the filesystem root.
	plan, err := Enumerate([]string{root, "/"})

	// Then: the run refuses.
	if err == nil {
		t.Fatal("a command line naming the filesystem root was accepted")
	}
	// And: the ordinary tree is not in the plan.
	if plan.Total() != 0 {
		t.Fatalf("a refused command line still planned %d targets: %v", plan.Total(), plan.Targets())
	}
}

// A path inside the home directory is an ordinary target.
// Given a project directory under home, when it is enumerated, then it is
// accepted and the file inside it is planned.
// Why it matters: the guard refuses a path that holds home, and the same
// comparison read backwards would refuse everything a person actually owns,
// which leaves the tool unable to destroy anything worth destroying.
func TestEnumerateAcceptsAPathInsideTheHomeDirectory(t *testing.T) {
	// Given: a home directory the test owns, holding a project.
	home := t.TempDir()
	t.Setenv("HOME", home)
	project := filepath.Join(home, "project")
	if err := os.MkdirAll(project, 0o700); err != nil {
		t.Fatalf("creating the project: %v", err)
	}
	note := filepath.Join(project, "note.txt")
	if err := os.WriteFile(note, []byte("payload"), 0o600); err != nil {
		t.Fatalf("writing the note: %v", err)
	}

	// When: the project is enumerated.
	plan, err := Enumerate([]string{project})

	// Then: it is not refused.
	if err != nil {
		t.Fatalf("a path inside the home directory was refused: %v", err)
	}
	// And: the file inside it is planned.
	if !slices.Contains(plan.Files, note) {
		t.Fatalf("the plan lists files %v, expected it to hold %s", plan.Files, note)
	}
}
