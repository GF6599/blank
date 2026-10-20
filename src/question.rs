use std::io::{self, BufRead, Write};

use crate::plan::Plan;
use crate::terminal::escape_path;

/// A glob can expand to thousands of arguments, and listing them all would scroll the counts off
/// the screen.
const ROOTS_NAMED: usize = 5;

/// Asks before anything is destroyed. Only a whole "yes" goes on, in any case and with any
/// padding, because the answer to a question nothing can undo should not be reachable by a stray
/// keystroke. A closed input reads as a refusal rather than a wait, so a command in a pipeline
/// stops instead of destroying a tree nobody confirmed.
pub fn confirm(input: &mut impl BufRead, output: &mut impl Write, plan: &Plan) -> io::Result<bool> {
    writeln!(output, "blank will destroy {}.", plan.summary())?;
    writeln!(output, "Selected roots:")?;
    for root in plan.roots().iter().take(ROOTS_NAMED) {
        writeln!(output, "  {}", escape_path(root))?;
    }
    if plan.roots().len() > ROOTS_NAMED {
        writeln!(
            output,
            "  ... and {} more",
            plan.roots().len() - ROOTS_NAMED
        )?;
    }
    write!(output, "This cannot be undone. Type yes to continue: ")?;
    output.flush()?;
    let mut answer = Vec::new();
    input.read_until(b'\n', &mut answer)?;
    writeln!(output)?;
    Ok(answer.trim_ascii().eq_ignore_ascii_case(b"yes"))
}
