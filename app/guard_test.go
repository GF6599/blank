package app

import (
	"os"
	"os/user"
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

// The home directory stays protected whatever the environment says.
// Given an environment that has lost HOME or points it somewhere else, when the
// real home directory is named, then the run still refuses it.
// Why it matters: HOME belongs to whoever started the process, and a run from
// cron or a systemd unit has a minimal environment. Reading only that variable
// drops the protection when it is missing and aims it at the wrong directory
// when it is wrong, and neither failure says anything on the way past.
func TestTheHomeGuardDoesNotDependOnTheEnvironment(t *testing.T) {
	// Given: a home directory that only the user database knows about.
	home := t.TempDir()
	currentUser = func() (*user.User, error) { return &user.User{HomeDir: home}, nil }
	t.Cleanup(func() { currentUser = user.Current })

	for name, env := range map[string]string{
		"the variable is missing":       "",
		"the variable points elsewhere": t.TempDir(),
	} {
		t.Run(name, func(t *testing.T) {
			t.Setenv("HOME", env)

			// When: the real home directory is named.
			plan, err := Enumerate([]string{home})

			// Then: the run refuses it and plans nothing.
			if err == nil {
				t.Fatalf("%s was accepted as a target", home)
			}
			if plan.Total() != 0 {
				t.Fatalf("a refused run planned %d targets", plan.Total())
			}
		})
	}
}

// A protected directory is refused under every name that reaches it.
// Given a symlink to the directory holding home, when a path through that
// symlink names home, then the run refuses it.
// Why it matters: a machine whose /home is a symlink has two names for the same
// directory, and a guard that compares strings protects only the one it was
// told about. Typing the other one destroys home with no refusal at all.
func TestAProtectedPathIsRefusedThroughASymlink(t *testing.T) {
	// Given: a home directory only the user database knows about.
	home := t.TempDir()
	currentUser = func() (*user.User, error) { return &user.User{HomeDir: home}, nil }
	t.Cleanup(func() { currentUser = user.Current })

	// And: a second name for the directory holding it.
	alias := filepath.Join(t.TempDir(), "alias")
	if err := os.Symlink(filepath.Dir(home), alias); err != nil {
		t.Fatalf("creating the alias: %v", err)
	}

	// When: home is named through the alias.
	aliased := filepath.Join(alias, filepath.Base(home))
	plan, err := Enumerate([]string{aliased})

	// Then: the run refuses it and plans nothing.
	if err == nil {
		t.Fatalf("%s reaches the home directory and was accepted", aliased)
	}
	if plan.Total() != 0 {
		t.Fatalf("a refused run planned %d targets", plan.Total())
	}
}

// A symlink to a protected path is removed rather than refused.
// Given a symlink pointing at the home directory, when it is named, then the
// run plans to unlink it.
// Why it matters: resolving the last component too would refuse every link that
// points somewhere protected, and unlinking one destroys nothing but the link.
func TestASymlinkToAProtectedPathIsStillRemovable(t *testing.T) {
	// Given: a home directory, and a symlink pointing straight at it.
	home := t.TempDir()
	currentUser = func() (*user.User, error) { return &user.User{HomeDir: home}, nil }
	t.Cleanup(func() { currentUser = user.Current })
	link := filepath.Join(t.TempDir(), "link-to-home")
	if err := os.Symlink(home, link); err != nil {
		t.Fatalf("creating the link: %v", err)
	}

	// When: the link is named.
	plan, err := Enumerate([]string{link})

	// Then: it is planned as a removal, and nothing else is.
	if err != nil {
		t.Fatalf("a symlink pointing at home was refused: %v", err)
	}
	if !slices.Equal(plan.Removals, []string{link}) {
		t.Fatalf("the plan removes %v, expected only %s", plan.Removals, link)
	}
	if len(plan.Files)+len(plan.Directories) != 0 {
		t.Fatalf("the plan followed the link: files %v, directories %v", plan.Files, plan.Directories)
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
