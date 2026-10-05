//! Questions on the terminal: a line read, a choice picked, a yes or no;
//! and the answers to `questions` asked there, or given as flags.

use crate::model::{self, answer};
use crate::questions;
use crate::store::Store;
use anyhow::{bail, Result};
use std::io::Write as _;

/// Ctrl-D on a terminal: a blank answer, `None`, which a question that needs
/// one refuses rather than asks again. Piped input that has run out stops
/// at the question it ran out at: nothing after it is taken as blank.
pub(crate) fn ran_out(question: &str) -> Result<Option<String>> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        bail!("the input ran out at {question}");
    }
    Ok(None)
}

/// The answer, or the default for a blank one; `None` for Ctrl-D.
pub(crate) fn read(question: &str, default: &str) -> Result<Option<String>> {
    let prompt = if default.is_empty() { format!("{question}: ") } else { format!("{question} [{default}]: ") };
    // A line editor, so the arrow keys move along what has been typed.
    let s = match rustyline::DefaultEditor::new()?.readline(&prompt) {
        Err(rustyline::error::ReadlineError::Eof) => return ran_out(question),
        r => r?,
    };
    let s = s.trim();
    Ok(Some(if s.is_empty() { default.to_string() } else { s.to_string() }))
}

/// The answer to a question put with `now` already on the line, to change
/// or keep; `None` for Ctrl-D. Without a terminal nothing can be put on the
/// line, so there a blank answer keeps `now`.
pub(crate) fn read_over(question: &str, now: &str) -> Result<Option<String>> {
    use std::io::IsTerminal as _;
    let s = match rustyline::DefaultEditor::new()?.readline_with_initial(&format!("{question}: "), (now, "")) {
        Err(rustyline::error::ReadlineError::Eof) => return ran_out(question),
        r => r?,
    };
    let s = s.trim();
    Ok(Some(if s.is_empty() && !std::io::stdin().is_terminal() { now.to_string() } else { s.to_string() }))
}

/// Asks until `check` accepts the answer, saying each time what was wrong.
pub(crate) fn prompt_until<T>(question: &str, default: &str, check: impl Fn(&str) -> Result<T, String>) -> Result<T> {
    ask_until(|| read(question, default), question, default, check)
}

/// `prompt_until`, with `now` on the line to change or keep.
pub(crate) fn prompt_over_until<T>(question: &str, now: &str, check: impl Fn(&str) -> Result<T, String>) -> Result<T> {
    ask_until(|| read_over(question, now), question, now, check)
}

pub(crate) fn ask_until<T>(
    read: impl Fn() -> Result<Option<String>>,
    question: &str,
    default: &str,
    check: impl Fn(&str) -> Result<T, String>,
) -> Result<T> {
    loop {
        let answer = read()?;
        match check(answer.as_deref().unwrap_or(default)) {
            Ok(v) => return Ok(v),
            // Ctrl-D is no answer to ask again for.
            Err(e) if answer.is_none() => bail!("{question}: {e}"),
            Err(e) => eprintln!("  {e}"),
        }
    }
}

/// Lines until an empty one or Ctrl-D, under the question, the first ones
/// starting as `now`'s, to change or keep.
pub(crate) fn prompt_lines_over(question: &str, now: &[&str]) -> Result<Vec<String>> {
    use std::io::IsTerminal as _;
    println!("{question}, a line at a time; an empty line ends it:");
    let mut editor = rustyline::DefaultEditor::new()?;
    let mut lines = vec![];
    loop {
        let now = now.get(lines.len()).copied().unwrap_or("");
        // Without a terminal there is no line to put them on.
        let now = if std::io::stdin().is_terminal() { now } else { "" };
        match editor.readline_with_initial("  ", (now, "")) {
            Err(rustyline::error::ReadlineError::Eof) => {
                ran_out(question)?;
                break;
            }
            r if r.as_ref().is_ok_and(|l| l.trim().is_empty()) => break,
            r => lines.push(r?.trim().to_string()),
        }
    }
    Ok(lines)
}

/// One of `options`, from a list on a terminal: up and down, or a letter for
/// the next option starting with it, then space or enter. Without a terminal
/// the answer is typed, and `check` reads it.
pub(crate) fn choose(
    question: &str,
    options: &[&'static str],
    default: &str,
    check: impl Fn(&str) -> Result<&'static str, String>,
) -> Result<&'static str> {
    #[cfg(unix)]
    {
        use std::io::IsTerminal as _;
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            return pick(question, options, default);
        }
    }
    prompt_until(&format!("{question} ({})", options.join(" / ")), default, check)
}

#[cfg(unix)]
pub(crate) fn pick(question: &str, options: &[&'static str], default: &str) -> Result<&'static str> {
    use nix::sys::termios::{tcgetattr, tcsetattr, LocalFlags, SetArg};

    /// Puts the terminal back however the choosing ends, a refusal included.
    struct Restore(nix::sys::termios::Termios);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = tcsetattr(std::io::stdin(), SetArg::TCSANOW, &self.0);
            print!("\x1b[?25h");
            let _ = std::io::stdout().flush();
        }
    }
    let saved = tcgetattr(std::io::stdin())?;
    let mut keys = saved.clone();
    // Key by key and unechoed, with Ctrl-C read as a key rather than a
    // signal, so that the terminal is put back before stopping.
    keys.local_flags.remove(LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG);
    tcsetattr(std::io::stdin(), SetArg::TCSANOW, &keys)?;
    let _restore = Restore(saved);

    let n = options.len();
    let mut at = options.iter().position(|o| *o == default).unwrap_or(0);
    let mut out = std::io::stdout();
    // The cursor is hidden while the marker shows where it is.
    writeln!(out, "\x1b[?25l{question} (up/down or a letter, then space):")?;
    'choosing: loop {
        for (i, o) in options.iter().enumerate() {
            writeln!(out, "\r\x1b[K  ({}) {o}", if i == at { '*' } else { ' ' })?;
        }
        out.flush()?;
        // Straight from the descriptor: a buffered read could take keys
        // typed ahead for the next question. One read may hold several keys.
        let mut buf = [0u8; 64];
        let len = nix::unistd::read(std::io::stdin(), &mut buf)?;
        if len == 0 {
            bail!("{question}: no answer");
        }
        let mut keys = &buf[..len];
        while !keys.is_empty() {
            let used = match keys {
                [b' ' | b'\n' | b'\r', ..] => break 'choosing,
                [0x1b, b'[' | b'O', b'A', ..] => { at = (at + n - 1) % n; 3 }
                [0x1b, b'[' | b'O', b'B', ..] => { at = (at + 1) % n; 3 }
                [0x03, ..] => bail!("interrupted"),
                [0x04, ..] => bail!("{question}: no answer"),
                [c, ..] if c.is_ascii_alphabetic() => {
                    let c = c.to_ascii_lowercase() as char;
                    if let Some(i) = (1..=n).map(|k| (at + k) % n).find(|&i| options[i].starts_with(c)) {
                        at = i;
                    }
                    1
                }
                _ => 1,
            };
            keys = &keys[used..];
        }
        write!(out, "\x1b[{n}A")?;
    }
    // The list gives way to one line, as every other question leaves.
    writeln!(out, "\x1b[{}A\r\x1b[J{question}: {}", n + 1, options[at])?;
    Ok(options[at])
}

/// Asks y/n on stderr, counting down; silence, or no terminal to ask on, is no.
pub(crate) fn confirm(question: &str, secs: u8) -> Result<bool> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    // The reader outlives a timeout, blocked on stdin, until the process ends.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::stdin().read_line(&mut s);
        let _ = tx.send(s);
    });
    let mut err = std::io::stderr();
    write!(err, "[{secs}] {question} [y/N] ")?;
    for left in (0..secs).rev() {
        err.flush()?;
        if let Ok(s) = rx.recv_timeout(std::time::Duration::from_secs(1)) {
            return Ok(matches!(s.trim(), "y" | "Y" | "yes"));
        }
        // Rewrite the count at the start of the line and put the cursor back
        // where it was, so what the user has typed stays put.
        write!(err, "\x1b7\r[{left}]\x1b8")?;
    }
    writeln!(err)?;
    Ok(false)
}

/// Answers from the terminal: each question put in turn, with what it
/// offers, or the answer being changed, already on the line to keep or edit.
pub(crate) struct Terminal {
    /// Answered on the command line: checked, not asked.
    pub(crate) given: questions::Answers,
    /// Answers being changed, each put on the line as it stands.
    pub(crate) start: questions::Answers,
}

/// A question's label with its hint, as the terminal puts it: "IBAN (blank
/// if none)".
pub(crate) fn question(label: &str, hint: &str) -> String {
    match hint.trim_end_matches('.') {
        "" => label.to_string(),
        h => format!("{label} ({}{})", h[..1].to_lowercase(), &h[1..]),
    }
}

impl questions::Source for Terminal {
    fn offer(&self, key: &str, offered: String) -> String {
        self.start.get(key).cloned().unwrap_or(offered)
    }

    fn note(&mut self, title: &str, note: &str) -> Result<()> {
        if note.is_empty() {
            return Ok(());
        }
        // Folded to the width of the other questions' words.
        let mut line = String::new();
        let note = format!("{title}: {}{}", note[..1].to_lowercase(), &note[1..]);
        for word in note.split_whitespace() {
            if !line.is_empty() && line.len() + word.len() >= 80 {
                println!("{line}");
                line.clear();
            }
            line = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        }
        if !line.is_empty() {
            println!("{line}");
        }
        Ok(())
    }

    fn text(
        &mut self,
        key: &str,
        label: &str,
        hint: &str,
        offer: &str,
        check: &dyn Fn(&str) -> Result<String, String>,
    ) -> Result<String> {
        if let Some(v) = self.given.get(key) {
            return check(v).map_err(anyhow::Error::msg);
        }
        let now = self.start.get(key).map_or(offer, String::as_str);
        prompt_over_until(&question(label, hint), now, check)
    }

    fn lines(&mut self, key: &str, label: &str, hint: &str) -> Result<String> {
        let now: Vec<&str> = self.start.get(key).map(|s| s.lines().collect()).unwrap_or_default();
        Ok(prompt_lines_over(&question(label, hint), &now)?.join("\n"))
    }

    fn choice(&mut self, key: &str, label: &str, noun: &str, options: &[&'static str], offer: &str) -> Result<String> {
        let now = self.start.get(key).map_or(offer, String::as_str);
        Ok(choose(label, options, now, |s| answer::one_of(s, options, noun))?.to_string())
    }

    fn another(&mut self, _key: &str, question: &str, n: usize, skipped: bool) -> Result<bool> {
        // The first is offered, and may be skipped; after one, another only
        // when asked for.
        Ok(match (n, skipped) {
            (0, _) => true,
            (_, true) => false,
            _ => prompt_until(question, "no", |s| answer::one_of(s, &["yes", "no"], "yes or no"))? == "yes",
        })
    }

    fn item_problem(&mut self, prefix: &str, _first: &str, problem: String, answers: &questions::Answers) -> Result<bool> {
        eprintln!("  {problem} Asking again.");
        self.start.extend(answers.iter().filter(|(k, _)| k.starts_with(prefix)).map(|(k, v)| (k.clone(), v.clone())));
        Ok(true)
    }
}

/// Answers given as flags: nothing is asked. What is not given is what the
/// question offers, as a blank answer is on the terminal; what is given is
/// checked as a typed answer is, and refused rather than asked again.
pub(crate) struct Flags {
    pub(crate) given: questions::Answers,
    /// What is wrong with the flags, each said, so that all are said at once.
    pub(crate) problems: Vec<String>,
    /// The items, by prefix, a field of which has a problem: the item's own
    /// problem would say it again.
    said: std::collections::BTreeSet<String>,
}

impl Flags {
    pub(crate) fn new(given: questions::Answers) -> Flags {
        Flags { given, problems: vec![], said: Default::default() }
    }

    /// Every problem the flags had, or none.
    pub(crate) fn done(self) -> Result<()> {
        if self.problems.is_empty() {
            return Ok(());
        }
        bail!("{}", self.problems.join("\n"))
    }
}

impl questions::Source for Flags {
    fn note(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    // Trimmed, as the terminal and the form take them: the same answers
    // open the same books, however given.
    fn text(&mut self, key: &str, label: &str, _: &str, _: &str, check: &dyn Fn(&str) -> Result<String, String>) -> Result<String> {
        let v = self.given.get(key).map_or("", String::as_str).trim().to_string();
        Ok(check(&v).unwrap_or_else(|e| {
            self.problems.push(format!("{label}: {e}"));
            self.said.insert(key.rsplit_once('.').map_or(String::new(), |(p, _)| format!("{p}.")));
            v
        }))
    }

    fn lines(&mut self, key: &str, _: &str, _: &str) -> Result<String> {
        let v = self.given.get(key).cloned().unwrap_or_default();
        Ok(v.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n"))
    }

    fn choice(&mut self, key: &str, label: &str, noun: &str, options: &[&'static str], offer: &str) -> Result<String> {
        match self.given.get(key) {
            None => Ok(offer.to_string()),
            Some(v) => Ok(answer::one_of(v, options, noun).map(str::to_string).unwrap_or_else(|e| {
                self.problems.push(format!("{label}: {e}"));
                offer.to_string()
            })),
        }
    }

    fn another(&mut self, key: &str, _: &str, n: usize, _: bool) -> Result<bool> {
        let prefix = format!("{key}{n}.");
        Ok(self.given.keys().any(|k| k.starts_with(&prefix)))
    }

    fn item_problem(&mut self, prefix: &str, _: &str, problem: String, _: &questions::Answers) -> Result<bool> {
        if self.said.contains(prefix) {
            return Ok(false);
        }
        let n: usize = prefix.trim_start_matches('p').trim_end_matches('.').parse().unwrap_or(0);
        self.problems.push(if prefix.is_empty() { problem } else { format!("--payment number {}: {problem}", n + 1) });
        Ok(false)
    }
}

/// A payment option's questions, answered by flags: what is not given is
/// what each offers.
pub(crate) fn flagged_option(s: &Store, given: questions::Answers, others: Vec<String>) -> Result<(String, model::PayOption)> {
    let context = questions::Context { root: None, ids: others, declared: Some(s.books.accounts.iter().cloned().collect()), ..Default::default() };
    let mut answers = questions::Answers::from([("name".into(), s.books.config.company.name.clone())]);
    let keys: Vec<String> = given.keys().cloned().collect();
    let mut flags = Flags::new(given);
    questions::ask_item(questions::OPTION, "", "skip", questions::option_problem, &mut answers, &context, &mut flags)?;
    // A flag no question of this kind asked is another kind's: refused, not
    // let go unsaid.
    if let Some(k) = keys.iter().find(|k| !answers.contains_key(*k)) {
        flags.problems.push(format!("a {} option has no {k}", answers.get("kind").map_or("", String::as_str)));
    }
    flags.done()?;
    Ok(questions::option(&answers, ""))
}

/// A payment option's questions, starting from `start`; `None` if skipped.
pub(crate) fn ask_option(s: &Store, start: questions::Answers, others: Vec<String>) -> Result<Option<(String, model::PayOption)>> {
    let context = questions::Context { root: None, ids: others, declared: Some(s.books.accounts.iter().cloned().collect()), ..Default::default() };
    // The company's name, offered as the account holder.
    let mut answers = questions::Answers::from([("name".into(), s.books.config.company.name.clone())]);
    let mut term = Terminal { given: Default::default(), start };
    if questions::ask_item(questions::OPTION, "", "skip", questions::option_problem, &mut answers, &context, &mut term)? {
        return Ok(None);
    }
    Ok(Some(questions::option(&answers, "")))
}

