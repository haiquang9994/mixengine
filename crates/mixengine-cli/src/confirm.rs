//! One question, asked of whoever is at the keyboard, and the three things that can come back.
//!
//! **The only place `mix` reads its own standard input**, and it exists for one command: T64 says
//! `mix elevation grant` prints every operation and what each will literally change *before* the
//! prompt is raised, and printing is only half of that — a person who has just read a list of
//! changes to their hosts file has to be able to act on it.
//!
//! **The decision is what can be read, not `IsTerminal`.** A pipe is not a terminal, so a rule
//! written around `IsTerminal` would refuse an answer somebody deliberately piped in and — worse —
//! could not be reached by any test, since `Command` hands its child a pipe and never a console.
//! End of file is the condition that actually matters: it is what a cron job, a CI step and a
//! service manager look like, and it is exactly the case that must not raise a dialog nobody is
//! there to see. See [`Answer::Unanswerable`].

use std::io::{BufRead as _, Write as _};

/// What came back from the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The word was typed.
    Yes,

    /// Anything else was, including nothing at all.
    No,

    /// There was nobody to ask: standard input is at end of file.
    ///
    /// Its own value rather than a [`No`](Self::No), because the two deserve different sentences.
    /// A person who answered no has decided; a script that could not be asked needs to be told
    /// which flag says yes in advance.
    Unanswerable,
}

/// What came back from a question with three answers — roadmap task **T78**.
///
/// A blueprint asking for a version this machine does not have is a question with three answers, and
/// the feature doc writes all three: *install it / use the installed one / cancel*. Its own type
/// rather than a second reading of [`Answer`], because "no" and "use what is here" are different
/// decisions and a yes/no that meant both would be a prompt nobody could answer correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Install what the blueprint asks for.
    Install,

    /// Use what this machine already has.
    UseInstalled,

    /// Neither, so nothing happens at all.
    Cancel,

    /// There was nobody to ask: standard input is at end of file.
    Unanswerable,
}

/// Ask one of those, and read one line back.
pub(crate) fn choose(question: &str) -> Choice {
    let mut error = std::io::stderr();
    let _ = write!(error, "{question}");
    let _ = error.flush();

    let mut line = String::new();

    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => {
            let _ = writeln!(error);

            chosen(None)
        }
        Ok(_) => chosen(Some(&line)),
    }
}

/// What one typed line means. [`None`] is end of file.
///
/// Split from [`choose`] on [`answer`]'s rule, so that the default — which is the one that matters —
/// is tested without a terminal to type into.
fn chosen(line: Option<&str>) -> Choice {
    let Some(line) = line else {
        return Choice::Unanswerable;
    };

    match line.trim().to_ascii_lowercase().as_str() {
        "i" | "install" => Choice::Install,
        "u" | "use" | "installed" => Choice::UseInstalled,

        // **The default is neither**, on [`answer`]'s reasoning one step further: a person who hits
        // Enter to see what happens has not chosen to download eighty megabytes, and has not chosen
        // to build their project on a version they did not ask for either.
        _ => Choice::Cancel,
    }
}

/// Put the question on standard error and read one line back.
///
/// Standard error for [`report_progress`](crate::report_progress)'s reason: what a command answers
/// with goes to standard output, and a question is not an answer. It also keeps a redirected
/// `mix … > file` readable, where a prompt written into the file would be a prompt nobody sees.
pub(crate) fn ask(question: &str) -> Answer {
    // Nothing to do about a stderr that will not take it: the read below still happens, and a
    // caller that cannot show the question is one whose answer is about to be `Unanswerable`
    // anyway. `write!` rather than `eprint!`, which panics when stderr is closed.
    let mut error = std::io::stderr();
    let _ = write!(error, "{question}");
    let _ = error.flush();

    let mut line = String::new();

    match std::io::stdin().lock().read_line(&mut line) {
        // Zero bytes and no error is end of file, which is the whole reason this function does not
        // ask `IsTerminal`.
        Ok(0) | Err(_) => {
            // A terminal echoes the newline a person types, so the question ends itself. Nothing
            // echoes an end of file, and whatever is said next would be printed onto the end of the
            // question that was never answered.
            let _ = writeln!(error);

            answer(None)
        }
        Ok(_) => answer(Some(&line)),
    }
}

/// What one typed line means. [`None`] is end of file.
///
/// Split from [`ask`] so that the rule — the default is no, and only the word is yes — is tested
/// without a terminal to type into.
fn answer(line: Option<&str>) -> Answer {
    let Some(line) = line else {
        return Answer::Unanswerable;
    };

    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Answer::Yes,
        _ => Answer::No,
    }
}

#[cfg(test)]
mod tests {
    use super::{Answer, Choice, answer, chosen};

    /// The default is no, and only the word means yes.
    ///
    /// What this is really about is the empty line: a person who hits Enter to see what happens has
    /// not agreed to anything, and a prompt that read that as consent would be raising an
    /// administrator's dialog on a keystroke.
    #[test]
    fn only_a_typed_yes_is_a_yes() {
        assert!(matches!(answer(Some("y\n")), Answer::Yes));
        assert!(matches!(answer(Some("Y")), Answer::Yes));
        assert!(matches!(answer(Some("  yes  \n")), Answer::Yes));
        assert!(matches!(answer(Some("YES")), Answer::Yes));

        assert!(matches!(answer(Some("n\n")), Answer::No));
        assert!(matches!(answer(Some("\n")), Answer::No));
        assert!(matches!(answer(Some("")), Answer::No));
        assert!(matches!(answer(Some("sure")), Answer::No));
    }

    /// End of file is a third thing, and the reason this returns three values rather than a `bool`.
    ///
    /// A cron job, a CI step and anything else with no terminal behind it reads this rather than a
    /// person's answer. Folding it into "no" would be nearly right and would hide the one sentence
    /// worth printing: pass `--yes`.
    #[test]
    fn a_closed_input_is_not_an_answer() {
        assert!(matches!(answer(None), Answer::Unanswerable));
    }

    /// Three answers and a default that is none of them — roadmap task T78.
    ///
    /// The empty line again, one question further along: a person who hits Enter has not chosen to
    /// download eighty megabytes, and has not chosen to build on a version they did not ask for.
    #[test]
    fn a_three_way_answer_defaults_to_cancelling() {
        assert_eq!(chosen(Some("i\n")), Choice::Install);
        assert_eq!(chosen(Some(" INSTALL ")), Choice::Install);
        assert_eq!(chosen(Some("u")), Choice::UseInstalled);
        assert_eq!(chosen(Some("installed\n")), Choice::UseInstalled);

        assert_eq!(chosen(Some("\n")), Choice::Cancel);
        assert_eq!(chosen(Some("what")), Choice::Cancel);
        assert_eq!(chosen(None), Choice::Unanswerable);
    }
}
