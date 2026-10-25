use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

use crate::terminal::escape;

pub const TOOL_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a running tool is checked, which bounds how long it outlives a cursor move.
const POLL: Duration = Duration::from_millis(20);
/// The longest side of a render, the side an image preview keeps.
const SIDE: &str = "1024";
/// Every tool reads the file the preview opened, as its standard input, so a name replaced after
/// the open cannot redirect the tool, and no file name can be read as an option.
const INPUT: &str = "/dev/stdin";
/// The bounds magick works within. Its own defaults allow most of the machine's memory, and an
/// SVG of a few hundred bytes can claim a canvas of gigapixels.
const MAGICK_LIMITS: [&str; 18] = [
    "-limit", "memory", "256MiB", "-limit", "map", "512MiB", "-limit", "disk", "1GiB", "-limit",
    "area", "128MP", "-limit", "width", "16KP", "-limit", "height", "16KP",
];
/// How much of a listing a preview reads. A tool still listing past it is killed, because an
/// archive of millions of entries lists for longer than the time limit, and the pane shows a few
/// dozen.
pub const LISTING_BYTES: u64 = 64 * 1024;
const MOST_MESSAGE_CHARS: usize = 200;
const MESSAGES_TAIL_BYTES: u64 = 4096;
/// The only coders magick may use. An SVG reaches any coder by a path to a file planted beside
/// it, and some coders read text, run drawing scripts, or write and delete files. MSVG draws
/// through MVG, and an SVG may embed a PNG, a JPEG or inline data. No delegate runs, so an SVG
/// never starts Inkscape.
const MAGICK_POLICY: &str = r#"<policymap>
  <policy domain="delegate" rights="none" pattern="*"/>
  <policy domain="path" rights="none" pattern="@*"/>
  <policy domain="coder" rights="none" pattern="*"/>
  <policy domain="coder" rights="read" pattern="{MSVG,MVG,HEIC,TTF,OTF,JPEG,INLINE}"/>
  <policy domain="coder" rights="read|write" pattern="PNG"/>
</policymap>
"#;
/// The ISO base media brands of a HEIF image. Every other brand is a video.
const HEIF_BRANDS: [&[u8]; 10] = [
    b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"hevm", b"hevs", b"mif1", b"msf1",
];

/// A kind of file that a tool outside blank draws or lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Video,
    Svg,
    Heic,
    Font,
    Archive(Archive),
}

/// An archive format that holds named entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Archive {
    Zip,
    SevenZip,
    Rar,
    Rar5,
    Tar,
}

impl Archive {
    fn of(head: &[u8], binary: bool) -> Option<Archive> {
        const ZIP: [&[u8]; 2] = [b"PK\x03\x04", b"PK\x05\x06"];
        const TAR_MAGIC: std::ops::Range<usize> = 257..262;
        if ZIP.iter().any(|magic| head.starts_with(magic)) {
            Some(Archive::Zip)
        } else if head.starts_with(b"7z\xbc\xaf\x27\x1c") {
            Some(Archive::SevenZip)
        } else if head.starts_with(b"Rar!\x1a\x07\x00") {
            Some(Archive::Rar)
        } else if head.starts_with(b"Rar!\x1a\x07\x01\x00") {
            Some(Archive::Rar5)
        } else if binary && head.get(TAR_MAGIC) == Some(b"ustar") {
            Some(Archive::Tar)
        } else {
            None
        }
    }

    /// The name 7zz gives the format.
    fn format(self) -> &'static str {
        match self {
            Archive::Zip => "zip",
            Archive::SevenZip => "7z",
            Archive::Rar => "rar",
            Archive::Rar5 => "rar5",
            Archive::Tar => "tar",
        }
    }

    pub fn caption(self) -> &'static str {
        match self {
            Archive::Zip => "ZIP archive",
            Archive::SevenZip => "7z archive",
            Archive::Rar | Archive::Rar5 => "RAR archive",
            Archive::Tar => "tar archive",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The name, escaped.
    pub path: String,
    pub size: Option<u64>,
    pub directory: bool,
}

impl Kind {
    /// The kind the head of a file shows, from its content and never its name. A video or a
    /// font is binary, and some of their marks are letters a text file can start with.
    pub fn of(head: &[u8]) -> Option<Kind> {
        let at = |range: std::ops::Range<usize>| head.get(range).unwrap_or_default();
        let binary = head.contains(&0);
        if head.starts_with(b"%PDF-") {
            Some(Kind::Pdf)
        } else if binary && at(4..8) == b"ftyp" {
            Some(if HEIF_BRANDS.contains(&at(8..12)) {
                Kind::Heic
            } else {
                Kind::Video
            })
        } else if binary && is_video(head) {
            Some(Kind::Video)
        } else if binary && [&[0, 1, 0, 0][..], b"OTTO", b"true", b"ttcf"].contains(&at(0..4)) {
            Some(Kind::Font)
        } else if let Some(archive) = Archive::of(head, binary) {
            Some(Kind::Archive(archive))
        } else if is_svg(head) {
            Some(Kind::Svg)
        } else {
            None
        }
    }

    pub fn tool(self) -> &'static str {
        match self {
            Kind::Pdf => "pdftoppm",
            Kind::Video => "ffmpegthumbnailer",
            Kind::Svg | Kind::Heic | Kind::Font => "magick",
            Kind::Archive(_) => "7zz",
        }
    }

    /// Whether the tool writes what it found on its standard output, rather than a picture.
    fn lists(self) -> bool {
        matches!(self, Kind::Archive(_))
    }

    /// What the pane says above the render, because the render is not the file itself.
    pub fn caption(self) -> &'static str {
        match self {
            Kind::Pdf => "PDF, first page",
            Kind::Video => "video, one frame",
            Kind::Svg => "SVG",
            Kind::Heic => "HEIC",
            Kind::Font => "font sample",
            Kind::Archive(archive) => archive.caption(),
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Kind::Pdf => "a PDF",
            Kind::Video => "a video",
            Kind::Svg => "an SVG",
            Kind::Heic => "a HEIC photo",
            Kind::Font => "a font",
            Kind::Archive(_) => "an archive",
        }
    }

    /// The arguments that draw the file on standard input as a PNG at `out`, or list it on
    /// standard output.
    fn args(self, out: &Path) -> Vec<OsString> {
        let owned = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        let mut args = Vec::new();
        match self {
            Kind::Pdf => {
                args.extend(owned(&[
                    "-f",
                    "1",
                    "-l",
                    "1",
                    "-singlefile",
                    "-png",
                    "-scale-to",
                    SIDE,
                    INPUT,
                ]));
                // pdftoppm adds the extension to the root it is given.
                args.push(out.with_extension("").into());
            }
            Kind::Video => {
                args.extend(owned(&["-i", INPUT, "-s", SIDE, "-c", "png", "-o"]));
                args.push(out.into());
            }
            Kind::Svg | Kind::Heic | Kind::Font => {
                // A coder named on the input makes magick read the file as that kind and no
                // other, whatever its content claims.
                let input = match self {
                    Kind::Svg => "msvg:/dev/stdin",
                    Kind::Heic => "heic:/dev/stdin[0]",
                    _ => "ttf:/dev/stdin",
                };
                let fit = format!("{SIDE}x{SIDE}>");
                args.extend(owned(&MAGICK_LIMITS));
                args.extend(owned(&[input, "-resize", &fit, "-depth", "8"]));
                let mut output = OsString::from("png:");
                output.push(out);
                args.push(output);
            }
            Kind::Archive(archive) => {
                // A password is given so that 7zz never asks for one, and the format is named
                // so that 7zz reads the file as that format and no other.
                let format = format!("-t{}", archive.format());
                args.extend(owned(&[
                    "l",
                    "-ba",
                    "-slt",
                    "-sccUTF-8",
                    "-p-",
                    &format,
                    INPUT,
                ]));
            }
        }
        args
    }
}

fn is_video(head: &[u8]) -> bool {
    const MATROSKA: &[u8] = &[0x1a, 0x45, 0xdf, 0xa3];
    const ASF: &[u8] = &[0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11];
    const MPEG_PROGRAM: &[u8] = &[0, 0, 1, 0xba];
    const MPEG_TRANSPORT_SYNC: u8 = 0x47;
    const MPEG_TRANSPORT_PACKET: usize = 188;
    [MATROSKA, ASF, MPEG_PROGRAM, b"FLV\x01"]
        .iter()
        .any(|magic| head.starts_with(magic))
        || (head.starts_with(b"RIFF") && head.get(8..12) == Some(b"AVI "))
        || (head.first() == Some(&MPEG_TRANSPORT_SYNC)
            && head.get(MPEG_TRANSPORT_PACKET) == Some(&MPEG_TRANSPORT_SYNC))
}

/// Whether the first element of the text, after any declaration, comment or doctype, is `<svg`.
fn is_svg(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        let end = if rest.starts_with("<?") {
            "?>"
        } else if rest.starts_with("<!--") {
            "-->"
        } else if rest.starts_with("<!") {
            ">"
        } else {
            return rest.starts_with("<svg");
        };
        let Some(at) = rest.find(end) else {
            return false;
        };
        rest = rest[at + end.len()..].trim_start();
    }
}

/// Runs the tools that draw or list a kind of file, and owns the processes and the private
/// directory they write in. `close` kills every tool still running and removes the directory.
pub struct Renderer {
    /// The search path for the tools, or the platform default when unset.
    search: Option<OsString>,
    state: Mutex<State>,
    next: AtomicU64,
}

#[derive(Default)]
struct State {
    closed: bool,
    dir: Option<PathBuf>,
    running: HashMap<u64, Child>,
}

impl Renderer {
    /// A renderer that finds the tools where the shell that started blank finds them.
    pub fn for_host() -> Renderer {
        Renderer::with_search(env::var_os("PATH"))
    }

    /// A renderer that finds the tools on `search` only.
    pub fn searching(search: impl AsRef<OsStr>) -> Renderer {
        Renderer::with_search(Some(search.as_ref().to_owned()))
    }

    fn with_search(search: Option<OsString>) -> Renderer {
        Renderer {
            search,
            state: Mutex::default(),
            next: AtomicU64::new(0),
        }
    }

    /// Draws or lists `input`, read from its first byte, with the tool of `kind`, and returns the
    /// PNG or the listing, already removed from the directory. It returns nothing once `stop` is
    /// set or the renderer is closed, and kills the tool then.
    pub fn render(
        &self,
        kind: Kind,
        mut input: File,
        stop: &AtomicBool,
    ) -> Option<Result<File, String>> {
        let tool = kind.tool();
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (out, messages) = {
            let mut state = self.state();
            if state.closed {
                return None;
            }
            match state.start(id, kind, &mut input, self.search.as_deref()) {
                Ok(paths) => paths,
                Err(reason) => return Some(Err(reason)),
            }
        };
        let deadline = Instant::now() + TOOL_TIMEOUT;
        let result = loop {
            thread::sleep(POLL);
            let mut state = self.state();
            // Only a close takes a child it did not start, and it has killed it.
            let child = state.running.get_mut(&id)?;
            match child.try_wait() {
                Ok(Some(status)) => {
                    state.running.remove(&id);
                    break finished(tool, status, &out, &messages);
                }
                Ok(None) if kind.lists() && listed_past_the_read(&out) => {
                    end(state.running.remove(&id)?);
                    break File::open(&out).map_err(|error| format!("cannot read: {error}"));
                }
                Ok(None) => {}
                Err(error) => {
                    end(state.running.remove(&id)?);
                    break Err(format!("cannot wait for {tool}: {error}"));
                }
            }
            let stopped = stop.load(Ordering::Relaxed);
            if stopped || Instant::now() >= deadline {
                end(state.running.remove(&id)?);
                if stopped {
                    drop(state);
                    remove(&[&out, &messages]);
                    return None;
                }
                let limit = TOOL_TIMEOUT.as_secs();
                break Err(format!(
                    "{tool} did not finish in {limit} s, and was stopped"
                ));
            }
        };
        remove(&[&out, &messages]);
        Some(result)
    }

    pub fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        for (_, child) in state.running.drain() {
            end(child);
        }
        if let Some(dir) = state.dir.take() {
            let _ = fs::remove_dir_all(dir);
        }
    }

    /// The state, also after a panic on another preview thread, so a close still kills.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.close();
    }
}

impl State {
    /// Starts the tool on `input` and returns where it writes its picture and its messages, or
    /// why it did not start.
    fn start(
        &mut self,
        id: u64,
        kind: Kind,
        input: &mut File,
        search: Option<&OsStr>,
    ) -> Result<(PathBuf, PathBuf), String> {
        let tool = kind.tool();
        let cannot = |error: io::Error| format!("cannot start {tool}: {error}");
        input.seek(SeekFrom::Start(0)).map_err(cannot)?;
        let dir = self.directory().map_err(cannot)?;
        let out = dir.join(format!("{id}.{}", if kind.lists() { "txt" } else { "png" }));
        let messages = dir.join(format!("{id}.log"));
        let stdout = if kind.lists() {
            Stdio::from(File::create(&out).map_err(cannot)?)
        } else {
            Stdio::null()
        };
        let mut command = Command::new(tool);
        command
            .args(kind.args(&out))
            .stdin(input.try_clone().map_err(cannot)?)
            .stdout(stdout)
            .stderr(File::create(&messages).map_err(cannot)?)
            // A tool that keeps a file of its own while it works keeps it here, so the close
            // removes what a killed tool could not.
            .env("TMPDIR", &dir)
            .env("MAGICK_TEMPORARY_PATH", &dir)
            .env("MAGICK_CONFIGURE_PATH", &dir)
            // Its own group, so a kill reaches every process it started, and the terminal never
            // takes it for the foreground job.
            .process_group(0);
        if let Some(search) = search {
            command.env("PATH", search);
        }
        let child = command.spawn().map_err(|error| {
            remove(&[&out, &messages]);
            if error.kind() == io::ErrorKind::NotFound {
                format!("install {tool} to preview {}", kind.noun())
            } else {
                cannot(error)
            }
        })?;
        self.running.insert(id, child);
        Ok((out, messages))
    }

    /// The private directory, made on the first render with a name nobody can guess and access
    /// for the owner only.
    fn directory(&mut self) -> io::Result<PathBuf> {
        if let Some(dir) = &self.dir {
            return Ok(dir.clone());
        }
        let base = env::temp_dir();
        loop {
            let dir = base.join(format!("blank-preview-{:016x}", rand::random::<u64>()));
            match DirBuilder::new().mode(0o700).create(&dir) {
                Ok(()) => {
                    if let Err(error) = fs::write(dir.join("policy.xml"), MAGICK_POLICY) {
                        let _ = fs::remove_dir_all(&dir);
                        return Err(error);
                    }
                    self.dir = Some(dir.clone());
                    return Ok(dir);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
    }
}

/// The picture or the listing of a finished tool, or the last thing it said when it failed.
fn finished(tool: &str, status: ExitStatus, out: &Path, messages: &Path) -> Result<File, String> {
    if !status.success() {
        let said = tail(messages).unwrap_or_default();
        let said = String::from_utf8_lossy(&said);
        let lines: Vec<&str> = said
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        // 7zz ends with a summary such as `Headers Error`, after the line that says what failed.
        let error = lines
            .iter()
            .rev()
            .find_map(|line| line.strip_prefix("ERROR: "));
        let last = error
            .map(|line| line.strip_prefix(&format!("{INPUT} : ")).unwrap_or(line))
            .or(lines.last().copied())
            .map(|line| escape(&line.chars().take(MOST_MESSAGE_CHARS).collect::<String>()));
        return Err(match last {
            Some(line) => format!("{tool} failed: {line}"),
            None => format!("{tool} failed with {status}"),
        });
    }
    File::open(out).map_err(|_| format!("{tool} drew no picture"))
}

/// The end of the messages a tool wrote, which a tool stuck on a damaged file can fill with
/// warnings for as long as it runs.
fn tail(messages: &Path) -> io::Result<Vec<u8>> {
    let mut file = File::open(messages)?;
    let length = file.metadata()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(MESSAGES_TAIL_BYTES)))?;
    let mut said = Vec::new();
    file.read_to_end(&mut said)?;
    Ok(said)
}

fn listed_past_the_read(out: &Path) -> bool {
    fs::metadata(out).is_ok_and(|metadata| metadata.len() > LISTING_BYTES)
}

/// The entries of a listing in the form `7zz l -slt` writes: a block of `name = value` lines for
/// each entry, and an empty line after it. 7zz writes a newline inside a name or a comment as
/// `_`, so no name can add a line of its own. A listing cut at `LISTING_BYTES` loses its last
/// entry, which can be cut too.
pub fn entries(listing: File) -> io::Result<Vec<Entry>> {
    let length = listing.metadata()?.len();
    let mut text = Vec::new();
    listing.take(LISTING_BYTES).read_to_end(&mut text)?;
    let text = String::from_utf8_lossy(&text);
    let mut blocks: Vec<&str> = text.split("\n\n").collect();
    if length > LISTING_BYTES {
        blocks.pop();
    }
    Ok(blocks.into_iter().filter_map(entry).collect())
}

fn entry(block: &str) -> Option<Entry> {
    let field = |name: &str| {
        block
            .lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(" = "))
    };
    Some(Entry {
        path: escape(field("Path")?),
        size: field("Size").and_then(|size| size.parse().ok()),
        directory: field("Folder") == Some("+")
            || field("Attributes").is_some_and(|attributes| attributes.starts_with('D')),
    })
}

/// Kills the tool and every process it started, then reaps it.
fn end(mut child: Child) {
    let group = Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
    let _ = killpg(group, Signal::SIGKILL);
    let _ = child.wait();
}

fn remove(paths: &[&Path]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}
