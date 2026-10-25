# blank demo, recorded with vhs (https://github.com/charmbracelet/vhs)
# Usage: just demo
#
# The demo destroys only what it creates under /tmp/blank-demo, which a
# hidden step builds fresh before every recording.

Output doc/demo.gif

Set FontSize 14
Set Width 1200
Set Height 700
Set Theme "Rose Pine"
Set Framerate 10
Set TypingSpeed 60ms

# Env comes after every Set, because vhs ignores a Set that follows any command.
Env BLANK_DEMO "/tmp/blank-demo"

Hide
Type `alias blank="$PWD/target/release/blank" && rm -rf "$BLANK_DEMO" && mkdir -p "$BLANK_DEMO/archive" "$BLANK_DEMO/keys"`
Enter
Type `printf 'Meeting notes\n- move the backups\n- rotate the keys\n' > "$BLANK_DEMO/notes.txt" && printf 'not a real key\n' > "$BLANK_DEMO/keys/id_demo"`
Enter
Type `head -c 300000 /dev/urandom > "$BLANK_DEMO/archive/backup.tar" && cd "$BLANK_DEMO" && clear`
Enter
Sleep 1s
Show

Type "blank"
Sleep 500ms
Enter
Sleep 2s

# Mark every entry, pausing on each so its preview shows
Space
Sleep 1s
Down
Sleep 1s
Space
Sleep 1s
Down
Sleep 1500ms
Space
Sleep 1s

Enter
Sleep 2500ms
Type "yes"
Sleep 800ms
Enter

# Watch the run, then leave the run view so the verdict prints
Sleep 3s
Type "q"
Sleep 2500ms
