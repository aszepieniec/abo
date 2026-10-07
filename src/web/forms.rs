//! The questions of `questions.rs` as a form: its fields filled in, and its answers read back, every problem said under its field.

use super::*;

/// Field name to what is wrong with the answer given there.
pub(super) type Problems = std::collections::BTreeMap<String, String>;

/// At most this many payment options on the form, however often another is
/// asked for.
pub(super) const MAX_OPTIONS: usize = 20;

/// The form as it starts: each choice and each fixed offer filled in, one
/// of each repeat's items.
pub(super) fn initial(qs: &[Q], prefix: &str, f: &mut Fields) {
    for q in qs {
        match q {
            Q::Group { qs, .. } | Q::When { qs, .. } => initial(qs, prefix, f),
            Q::Text { key, offer: questions::Offer::Value(v), .. } => {
                f.insert(format!("{prefix}{key}"), v.to_string());
            }
            Q::Text { .. } | Q::Lines { .. } => {}
            Q::Choice { key, options, offer, .. } => {
                f.insert(format!("{prefix}{key}"), offer.to_string());
                for (_, then) in options.iter() {
                    initial(then, prefix, f);
                }
            }
            Q::Repeat { key, item, .. } => {
                f.insert(format!("{prefix}{key}.n"), "1".into());
                initial(item, &format!("{prefix}{key}0."), f);
            }
        }
    }
}

/// Answers from a posted form. A wrong one is noted under its field, and
/// the rest are still read, so that every problem is said at once.
pub(super) struct Posted<'a> {
    pub(super) f: &'a Fields,
    pub(super) problems: Problems,
}

impl Posted<'_> {
    fn raw(&self, key: &str) -> String {
        self.f.get(key).map(|s| s.trim().to_string()).unwrap_or_default()
    }
}

impl questions::Source for Posted<'_> {
    fn note(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }

    /// Editing an option (`was` names it), a field sent blank stays blank,
    /// as on the terminal: the form showed what it holds, blank included.
    fn offer(&self, _key: &str, offered: String) -> String {
        if self.f.get("was").is_some_and(|w| !w.is_empty()) { String::new() } else { offered }
    }

    fn text(&mut self, key: &str, _: &str, _: &str, _: &str, check: &dyn Fn(&str) -> Result<String, String>) -> Result<String> {
        let raw = self.raw(key);
        Ok(check(&raw).unwrap_or_else(|e| {
            self.problems.insert(key.to_string(), e);
            raw
        }))
    }

    fn lines(&mut self, key: &str, _: &str, _: &str) -> Result<String> {
        // Blank lines dropped: on the terminal a blank line ends the answer.
        let v = self.raw(key).lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n");
        // Said under its field, with the rest of what is wrong, and kept
        // out of what is checked further.
        if v.chars().any(|c| crate::model::hidden(c) && c != '\n') {
            self.problems.insert(key.to_string(), format!("{v:?} holds a control character, or one that hides text."));
            return Ok(v.chars().filter(|c| !crate::model::hidden(*c) || *c == '\n').collect());
        }
        Ok(v)
    }

    fn choice(&mut self, key: &str, _: &str, noun: &str, options: &[&'static str], offer: &str) -> Result<String> {
        let raw = self.raw(key);
        match options.iter().find(|o| **o == raw) {
            Some(o) => Ok(o.to_string()),
            None => {
                self.problems.insert(key.to_string(), format!("Choose {noun}: {}.", options.join(", ")));
                Ok(offer.to_string())
            }
        }
    }

    fn another(&mut self, key: &str, _: &str, n: usize, _: bool) -> Result<bool> {
        let items: usize = self.raw(&format!("{key}.n")).parse().unwrap_or(1);
        Ok(n < items.clamp(1, MAX_OPTIONS))
    }

    fn item_problem(&mut self, prefix: &str, key: &str, problem: String, _: &questions::Answers) -> Result<bool> {
        // Under the field it is in, unless a field of the item already has a
        // problem that explains it.
        if !self.problems.keys().any(|k| k.starts_with(prefix)) {
            self.problems.insert(key.to_string(), problem);
        }
        Ok(false)
    }
}

/// The questions as a form, filled in from `f`, each problem under its
/// field. What follows a choice shows once it is chosen, through CSS rules
/// written into `css` as the form is drawn: no script. Where `:has` is not
/// supported every field shows, and the ones not chosen are ignored. A
/// choice's options in `omit` are left out.
pub(super) fn render(qs: &[Q], prefix: &str, f: &Fields, problems: &Problems, omit: &[&str], css: &mut Vec<String>) -> Markup {
    let v = |k: &str| f.get(k).cloned().unwrap_or_default();
    let problem = |k: &str| html! { @if let Some(p) = problems.get(k) { span.problem { (p) } } };
    let mut out = html! {};
    for q in qs {
        let m = match q {
            // Who keeps the books: asked in words of the page's own, folded
            // away, as a blank answer takes what the computer is set up with.
            Q::Group { title: "git", qs, .. } => html! {
                details.box {
                    summary.box-header { (icon("pencil")) "Who keeps the books (optional)" }
                    div.box-body {
                        p.muted.small { "Their name and email go with every change made to the books, to show who made it. Blank takes the ones this computer is set up with." }
                        (render(qs, prefix, f, problems, omit, css))
                    }
                }
            },
            Q::Group { title, icon: ic, note, qs } => html! {
                div.box {
                    div.box-header { (icon(ic)) (title) }
                    div.box-body {
                        // How its books are named on disk is the computer's business.
                        @if !note.is_empty() && *title != "The company" { p.muted.small { (with_code(note)) } }
                        (render(qs, prefix, f, problems, omit, css))
                    }
                }
            },
            Q::Text { key, label, hint, offer, .. } => {
                let k = format!("{prefix}{key}");
                let blank = match offer {
                    _ if key.starts_with("git_") => String::new(),
                    questions::Offer::From(words, _) => format!("Blank for {words}."),
                    _ => String::new(),
                };
                let label = match *key {
                    "git_name" => "Name",
                    "git_email" => "Email",
                    _ => label,
                };
                html! {
                    label.field { (label)
                        @if !hint.is_empty() || !blank.is_empty() { span.hint { (hint) " " (blank) } }
                        // An account field suggests the declared accounts, where
                        // the page has them as #accounts.
                        input type="text" name=(k) value=(v(&k)) list=[key.ends_with("account").then_some("accounts")];
                        (problem(&k))
                    }
                }
            }
            Q::Lines { key, label, hint } => {
                let k = format!("{prefix}{key}");
                html! {
                    label.field { (label)
                        span.hint { (hint) " One line each." }
                        textarea name=(k) rows="3" { (v(&k)) }
                        (problem(&k))
                    }
                }
            }
            Q::Choice { key, label, options, .. } => {
                let k = format!("{prefix}{key}");
                let branches: Vec<(String, &[Q])> = options
                    .iter()
                    .filter(|(_, then)| !then.is_empty())
                    .map(|(o, then)| (shown_when(css, &k, &[o]), *then))
                    .collect();
                html! {
                    label.field { (label)
                        select name=(k) {
                            @for (o, _) in options.iter().filter(|(o, _)| !omit.contains(o)) {
                                option value=(o) selected[v(&k) == *o] { (choice_said(key, o)) }
                            }
                        }
                        (problem(&k))
                    }
                    @for (class, then) in &branches {
                        div class=(class) { (render(then, prefix, f, problems, omit, css)) }
                    }
                }
            }
            Q::When { key, is, qs } => {
                let class = shown_when(css, &format!("{prefix}{key}"), is);
                html! { div class=(class) { (render(qs, prefix, f, problems, omit, css)) } }
            }
            Q::Repeat { key, item, another, .. } => {
                let n = v(&format!("{prefix}{key}.n")).parse().unwrap_or(1usize).clamp(1, MAX_OPTIONS);
                html! {
                    @for i in 0..n {
                        div.option { (render(item, &format!("{prefix}{key}{i}."), f, problems, omit, css)) }
                    }
                    input type="hidden" name={ (prefix) (key) ".n" } value=(n);
                    @if n < MAX_OPTIONS {
                        div.actions { button type="submit" name="more" value={ (prefix) (key) } { (icon("plus")) " " (another) } }
                    }
                }
            }
        };
        out = html! { (out) (m) };
    }
    out
}

/// Words, with what is between backticks set as code.
pub(super) fn with_code(s: &str) -> Markup {
    html! { @for (i, part) in s.split('`').enumerate() { @if i % 2 == 1 { code { (part) } } @else { (part) } } }
}

/// A class for markup shown only while one of `options` is chosen at `key`,
/// and the rule that does it.
pub(super) fn shown_when(css: &mut Vec<String>, key: &str, options: &[&str]) -> String {
    let class = format!("q{}", css.len());
    let chosen: Vec<String> = options.iter().map(|o| format!("select[name=\"{key}\"] option[value=\"{o}\"]:checked")).collect();
    css.push(format!("form:not(:has({})) .{class} {{ display: none; }}", chosen.join(", ")));
    class
}

pub(super) fn find_repeat<'a>(qs: &'a [Q], key: &str) -> Option<&'a Q> {
    qs.iter().find_map(|q| match q {
        Q::Repeat { key: k, .. } if *k == key => Some(q),
        Q::Group { qs, .. } | Q::When { qs, .. } => find_repeat(qs, key),
        Q::Choice { options, .. } => options.iter().find_map(|(_, then)| find_repeat(then, key)),
        _ => None,
    })
}

/// A choice as a page names it: the VAT methods by what they are, their
/// short name beside, as the company page says them.
fn choice_said(key: &str, o: &str) -> String {
    match (key, o) {
        ("method", "effective") => "As charged and paid (effective)".into(),
        ("method", "flat") => "Flat rate (flat)".into(),
        ("method", "none") => "Not registered (none)".into(),
        _ => o.to_string(),
    }
}
