//! The corporate documents (MODEL.md §10, G-C1 to G-C3): a tree under
//! `corporate/` that the company arranges as it likes, and that abo reads
//! nothing of. Its commands are not operations: each changes the tree alone,
//! as one commit, logged as the command that makes it again.

use crate::repo;
use crate::store::{self, Store};
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub const ROOT: &str = "corporate";
/// In every directory under `corporate/`, so that one emptied stays: git
/// keeps no directory without a file.
pub const KEEP: &str = ".keep";

/// A refusal of a corporate command: the change cannot be made as asked.
fn refused(what: String) -> anyhow::Error {
    store::Refused(vec![crate::model::Violation::new("OP", what)]).into()
}

/// A name as a refusal shows it: between quotes, so that a space it ends
/// with shows, and cut short where it is long.
fn shown(name: &str) -> String {
    let n = crate::model::answer::plain(name);
    match n.char_indices().nth(40) {
        Some((i, _)) => format!("\u{201c}{}\u{2026}\u{201d}", &n[..i]),
        None => format!("\u{201c}{n}\u{201d}"),
    }
}

/// `s` in one form, NFC: `ä` one character, as it is typed, not `a` and a
/// mark, as a Mac writes it in a file's name.
pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}

/// What two names are the same as, where case is ignored: `ß`, `ẞ` and
/// `SS`, `Σ`, `σ` and `ς`; and `ä` however it is written. Lower, upper,
/// lower: `ẞ` lowers to `ß`, which only upper case makes `SS`.
pub fn folded(name: &str) -> String {
    nfc(&nfc(name).to_lowercase().to_uppercase().to_lowercase())
}

/// Why `name` cannot be a name under `corporate/` (MODEL.md §3, Corporate
/// documents), if it cannot.
pub fn bad_name(name: &str) -> Option<String> {
    let q = shown(name);
    if name.is_empty() {
        Some("a name is empty, as between two slashes".into())
    } else if name.len() > 255 {
        Some(format!("{q} is longer than 255 bytes"))
    } else if name.contains('/') {
        Some(format!("{q} holds a slash"))
    } else if name.contains('\\') {
        Some(format!("{q} holds a backslash"))
    } else if name.chars().any(crate::model::hidden) {
        Some(format!("{q} holds a character that does not show"))
    } else if name.chars().next().is_some_and(|c| matches!(unicode_general_category::get_general_category(c), unicode_general_category::GeneralCategory::NonspacingMark | unicode_general_category::GeneralCategory::SpacingMark | unicode_general_category::GeneralCategory::EnclosingMark)) {
        Some(format!("{q} begins with a mark, which belongs on a letter before it"))
    } else if name.starts_with('.') {
        Some(format!("{q} begins with a dot"))
    } else if name.ends_with(char::is_whitespace) {
        Some(format!("{q} ends with a space"))
    } else if name.ends_with('.') {
        Some(format!("{q} ends with a dot"))
    } else {
        None
    }
}

/// `path`, a path in the books, as its names under `corporate/`: none for
/// `corporate/` itself.
pub fn names(path: &str) -> Result<Vec<String>> {
    let path = &nfc(path);
    let shape = || refused(format!("{}: a corporate document's path begins with corporate/, as in corporate/minutes", if path.is_empty() { "no path is given".into() } else { shown(path) }));
    let path = path.strip_suffix('/').unwrap_or(path);
    let Some(rest) = path.strip_prefix(ROOT) else { return Err(shape()) };
    if rest.is_empty() {
        return Ok(vec![]);
    }
    let Some(rest) = rest.strip_prefix('/') else { return Err(shape()) };
    let names: Vec<String> = rest.split('/').map(String::from).collect();
    for n in &names {
        if let Some(why) = bad_name(n) {
            return Err(refused(format!("{why}, which no name under corporate/ may")));
        }
    }
    Ok(names)
}

/// `path`, which `mv` or `rm` takes away: one the last commit holds, a file
/// or a directory, whatever its names; a name committed by hand that abo
/// would not give is taken away all the same.
fn there(t: &Tree, path: &str, verb: &str) -> Result<String> {
    let p = path.strip_suffix('/').unwrap_or(path);
    if p != path && (t.is_file(p) || t.is_link(p)) {
        return Err(refused(format!("{p} is a file, not a directory, as the / after it says")));
    }
    if p.starts_with("corporate/") && (t.is_file(p) || t.is_link(p) || t.is_dir(p)) {
        return Ok(p.to_string());
    }
    if let Some(w) = t.written(p).filter(|_| p.starts_with("corporate/")) {
        if p != path && (t.is_file(&w) || t.is_link(&w)) {
            return Err(refused(format!("{w} is a file, not a directory, as the / after it says")));
        }
        return Ok(w);
    }
    let n = names(path)?;
    if n.is_empty() {
        return Err(refused(format!("corporate/ itself is not {verb}")));
    }
    Err(crate::input::not_found(format!("there is no {}", join(&n))))
}

fn join(names: &[String]) -> String {
    std::iter::once(ROOT).chain(names.iter().map(String::as_str)).collect::<Vec<_>>().join("/")
}

/// The tree as the last commit holds it: each file's path and size, and
/// apart, each symbolic link committed by hand, which is no file of it.
/// Paths, a set of them.
type Paths = std::collections::BTreeSet<String>;

fn entries(root: &Path) -> Result<(BTreeMap<String, u64>, Paths, Paths)> {
    let out = repo::git_in(root, &["ls-tree", "-r", "-l", "-z", "HEAD", "--", ROOT])?;
    let (mut files, mut links, mut exec) = (BTreeMap::new(), std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
    for l in out.split('\0') {
        let Some((meta, path)) = l.split_once('\t') else { continue };
        let mut m = meta.split_whitespace();
        let (Some(mode), Some(kind), Some(size)) = (m.next(), m.next(), m.nth(1)) else { continue };
        match (kind, mode) {
            ("blob", "120000") => drop(links.insert(path.to_string())),
            ("blob", m) => {
                if m == "100755" {
                    exec.insert(path.to_string());
                }
                files.insert(path.to_string(), size.parse().unwrap_or(0));
            }
            _ => {}
        }
    }
    Ok((files, links, exec))
}

/// The files of the tree as the last commit holds it, with their sizes.
pub fn files(root: &Path) -> Result<BTreeMap<String, u64>> {
    Ok(entries(root)?.0)
}

/// Whether some name of `path` under `corporate/` is one abo would not
/// give: one committed by hand.
fn by_hand(path: &str) -> bool {
    path.split('/').skip(1).any(|n| bad_name(n).is_some())
}

/// What the tree holds, for `list`: its directories, and its files with
/// their sizes, `.keep` left out.
#[derive(serde::Serialize)]
pub struct Listing {
    pub directories: Vec<String>,
    pub files: Vec<File>,
}

#[derive(serde::Serialize)]
pub struct File {
    pub path: String,
    pub size: u64,
}

pub fn list(root: &Path) -> Result<Listing> {
    let all = files(root)?;
    let mut directories = std::collections::BTreeSet::new();
    for p in all.keys() {
        let mut at = p.as_str();
        while let Some((d, _)) = at.rsplit_once('/') {
            if d == ROOT {
                break;
            }
            directories.insert(d.to_string());
            at = d;
        }
    }
    // `.keep`, and whatever else begins with a dot, there or above.
    let shown = |p: &str| !p.split('/').any(|n| n.starts_with('.'));
    let files = all.into_iter().filter(|(p, _)| shown(p)).map(|(path, size)| File { path, size }).collect();
    Ok(Listing { directories: directories.into_iter().filter(|d| shown(d)).collect(), files })
}

/// The tree, and what each command reads of it.
struct Tree {
    files: BTreeMap<String, u64>,
    links: std::collections::BTreeSet<String>,
    /// The files committed executable, whose mode a move keeps.
    exec: std::collections::BTreeSet<String>,
    /// Every directory the paths are in, found once, not by a scan each time.
    dirs: std::collections::HashSet<String>,
}

impl Tree {
    fn read(root: &Path) -> Result<Tree> {
        let (files, links, exec) = entries(root)?;
        let dirs = files.keys().chain(&links).flat_map(|p| Path::new(p).ancestors().skip(1).map(|d| d.to_string_lossy().into_owned()).take_while(|d| !d.is_empty()).collect::<Vec<_>>()).collect();
        Ok(Tree { files, links, exec, dirs })
    }
    fn paths(&self) -> impl Iterator<Item = &String> {
        self.files.keys().chain(&self.links)
    }
    fn is_file(&self, p: &str) -> bool {
        self.files.contains_key(p)
    }
    fn is_link(&self, p: &str) -> bool {
        self.links.contains(p)
    }
    fn is_dir(&self, p: &str) -> bool {
        p == ROOT || self.dirs.contains(p)
    }
    /// The path in the tree that is `p` written another way, `ä` as `a` and
    /// a mark, if one is: a file, a link, or a directory.
    fn written(&self, p: &str) -> Option<String> {
        let want = nfc(p);
        self.paths().find_map(|k| {
            let parts: Vec<&str> = k.split('/').collect();
            // Below `corporate/`, never `corporate` itself.
            (2..=parts.len()).map(|i| parts[..i].join("/")).find(|q| nfc(q) == want)
        })
    }
    fn under(&self, p: &str) -> Vec<String> {
        self.paths().filter(|k| *k == p || k.strip_prefix(p).is_some_and(|r| r.starts_with('/'))).cloned().collect()
    }
    /// The name in directory `dir` the same as `name` but for case, if one
    /// is there: a file, a link, or a directory.
    fn taken(&self, dir: &str, name: &str) -> Option<String> {
        self.taken_but(dir, name, "")
    }
    /// `taken`, `but` aside: what is moved is not in its own way. Every
    /// name there is looked at, not the first alone: two the same once
    /// folded may both be committed by hand.
    fn taken_but(&self, dir: &str, name: &str, but: &str) -> Option<String> {
        self.paths()
            .filter_map(|k| k.strip_prefix(dir)?.strip_prefix('/')?.split('/').next())
            .map(|n| format!("{dir}/{n}"))
            .find(|p| p != but && folded(p.rsplit('/').next().unwrap()) == folded(name))
    }
}

/// What a command writes: files put and removed, and the `.keep` of every
/// directory it needs, each new one made.
#[derive(Default)]
struct Change {
    puts: Vec<(String, Vec<u8>)>,
    removes: Vec<String>,
    /// What `mv` or `rm` takes away: files, and directories with all under them.
    gone: Vec<String>,
    /// What is put executable, as it was where it was moved from.
    exec: Vec<String>,
}

impl Change {
    /// Each directory from `corporate/` down to `dir` (names under it), made
    /// where it is not, with its `.keep`; refused where a file is there.
    fn dirs(&mut self, t: &Tree, dir: &[String]) -> Result<()> {
        for i in 1..=dir.len() {
            let d = join(&dir[..i]);
            if t.is_file(&d) {
                return Err(refused(format!("{d} is a file, not a directory")));
            }
            let keep = format!("{d}/{KEEP}");
            if !t.is_file(&keep) && !self.puts.iter().any(|(p, _)| *p == keep) {
                if !t.is_dir(&d) {
                    if let Some(other) = t.taken(&join(&dir[..i - 1]), &dir[i - 1]) {
                        return Err(clash(&d, &other));
                    }
                }
                self.puts.push((keep, vec![]));
            }
        }
        Ok(())
    }
}

/// Refused, as `path` is to go where `other` is.
fn clash(path: &str, other: &str) -> anyhow::Error {
    if path == other {
        let rm = crate::command::shell(&["corporate", "rm", "--", other]);
        refused(format!("{other} is there already: `{rm}` takes it out, to put another in its place"))
    } else {
        refused(format!("{path}: {other} is there already, and two names in a directory may not differ only in case, or in how a letter is written"))
    }
}

/// No symbolic link on the way to `rel`, nor at it, which taking it out
/// would follow out of `corporate/`.
fn no_link(root: &Path, rel: &str) -> Result<()> {
    let mut at = root.to_path_buf();
    for (i, p) in rel.split('/').enumerate() {
        at.push(p);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(refused(format!("{} is a symbolic link, which abo does not follow: take it out first", rel.split('/').take(i + 1).collect::<Vec<_>>().join("/"))));
        }
    }
    Ok(())
}

/// Nothing on disk where `rel` is to be written but what the last commit
/// holds: no directory on its way a symbolic link or a file, which writing
/// would follow out of `corporate/`, and no file there not committed.
fn clear(root: &Path, rel: &str, t: &Tree) -> Result<()> {
    let mut at = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for (i, p) in parts.iter().enumerate() {
        at.push(p);
        let Ok(meta) = std::fs::symlink_metadata(&at) else { return Ok(()) };
        let last = i + 1 == parts.len();
        if meta.is_dir() && at.join(".git").exists() {
            return Err(refused(format!("{} is a git repository of its own, which abo does not write into: take it out first", parts[..=i].join("/"))));
        }
        if meta.file_type().is_symlink() || (!last && !meta.is_dir()) || (last && !t.is_file(rel)) {
            let at = parts[..=i].join("/");
            return Err(refused(if !last && t.is_file(&at) {
                format!("{at} is a file, not a directory")
            } else if meta.file_type().is_symlink() {
                format!("{at} is a symbolic link, which abo does not follow: take it out first")
            } else {
                format!("{at} is there, and not committed: commit it yourself (git add, then git commit), or take it out first")
            }));
        }
    }
    Ok(())
}

fn commit(s: &mut Store, mut c: Change, subject: &str, argv: Vec<String>) -> Result<String> {
    s.changeable()?;
    let t = Tree::read(&s.root)?;
    // Every directory there is after the change holds `.keep` (MODEL.md
    // §10): one `mv` brings, and one made by hand without it that the
    // change is in, too.
    let gone = |p: &str| c.gone.iter().any(|g| p == g || p.strip_prefix(g.as_str()).is_some_and(|r| r.starts_with('/')));
    // What stays, and of it, once over: the directories kept already, by a
    // `.keep` in any capitals, a file or a directory committed by hand; and
    // each directory's names, folded, to find twins. Sets, not scans: a
    // directory of thousands moved is thousands of paths each way.
    let removed: std::collections::HashSet<&str> = c.removes.iter().map(String::as_str).collect();
    let mut kept: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut kids: BTreeMap<(String, String), std::collections::BTreeSet<String>> = BTreeMap::new();
    let tally = |p: &str, kids: &mut BTreeMap<(String, String), std::collections::BTreeSet<String>>, kept: &mut std::collections::HashSet<String>| {
        let parts: Vec<&str> = p.split('/').collect();
        let mut parent = parts[0].to_string();
        for name in &parts[1..] {
            if folded(name) == folded(KEEP) {
                kept.insert(parent.clone());
            }
            kids.entry((parent.clone(), folded(name))).or_default().insert(name.to_string());
            parent = format!("{parent}/{name}");
        }
    };
    for p in t.paths().filter(|p| !removed.contains(p.as_str())) {
        tally(p, &mut kids, &mut kept);
    }
    // Twins as the tree holds them before the change: one emptied by it is
    // a twin still, and goes, as nothing is written into it.
    let mut before: BTreeMap<(String, String), std::collections::BTreeSet<String>> = BTreeMap::new();
    for p in t.paths() {
        tally(p, &mut before, &mut std::collections::HashSet::new());
    }
    // A directory with a twin, committed by hand, is theirs to settle:
    // nothing is written into it, not even a `.keep`, until one goes.
    let twin = |d: &str, kids: &BTreeMap<(String, String), std::collections::BTreeSet<String>>| {
        let parts: Vec<&str> = d.split('/').collect();
        (1..parts.len()).any(|i| kids.get(&(parts[..i].join("/"), folded(parts[i]))).is_some_and(|k| k.len() > 1))
    };
    // Every directory there is after the change holds `.keep` (MODEL.md
    // §10): one `mv` brings, and one made by hand without it, too; but not
    // one with a name abo would not give, which it writes no path into
    // (G-C1), nor a twin's.
    let dirs: std::collections::BTreeSet<String> = t
        .paths()
        .chain(c.puts.iter().map(|(p, _)| p))
        .flat_map(|p| Path::new(p).ancestors().skip(1).map(|d| d.to_string_lossy().into_owned()).take_while(|d| d != ROOT).collect::<Vec<_>>())
        .filter(|d| !gone(d) && !by_hand(d) && !twin(d, &kids) && !(twin(d, &before) && !kids.keys().any(|(parent, _)| parent == d)))
        .collect();
    let asked: std::collections::HashSet<String> = c.puts.iter().map(|(p, _)| p.clone()).collect();
    for d in dirs {
        let keep = format!("{d}/{KEEP}");
        // Something not committed there already holds nothing up: the
        // directory goes without, until it is settled.
        let not_committed = std::fs::symlink_metadata(s.root.join(&keep)).is_ok() && !t.is_file(&keep);
        if !kept.contains(&d) && !asked.contains(&keep) && !not_committed {
            c.puts.push((keep, vec![]));
        }
    }
    // A `.keep` asked for where one in other capitals is kept already, or
    // comes with what is moved: a `.Keep` committed by hand.
    for (p, _) in &c.puts {
        if let Some((d, n)) = p.rsplit_once('/') {
            if n != KEEP && folded(n) == folded(KEEP) {
                kept.insert(d.to_string());
            }
        }
    }
    c.puts.retain(|(p, _)| match p.rsplit_once('/') {
        Some((d, KEEP)) => (!kept.contains(d) || t.is_file(p)) && !(std::fs::symlink_metadata(s.root.join(p)).is_ok() && !t.is_file(p)),
        _ => true,
    });
    // Each name a path written makes is one abo would give, whatever wrote
    // it — `mv` carrying names committed by hand too — but a directory the
    // tree holds already, and a `.keep` in any capitals; and none has a
    // twin beside it once folded: no two directories made one, no twins
    // brought in, none written into.
    for (p, _) in &c.puts {
        tally(p, &mut kids, &mut kept);
    }
    for (p, _) in &c.puts {
        let parts: Vec<&str> = p.split('/').collect();
        for i in 1..parts.len() {
            let (at, name) = (parts[..=i].join("/"), parts[i]);
            let old = t.is_dir(&at) && i + 1 < parts.len();
            let keep = i + 1 == parts.len() && folded(name) == folded(KEEP);
            if !old && !keep {
                if let Some(why) = bad_name(name) {
                    return Err(refused(format!("{why}, which no name under corporate/ may: {at} would be written so; rename it or take it out first")));
                }
            }
            let same = &kids[&(parts[..i].join("/"), folded(name))];
            if same.len() > 1 {
                let them: Vec<String> = same.iter().map(|n| format!("{}/{n}", parts[..i].join("/"))).collect();
                return Err(refused(format!("{} would be one name: two names in a directory may not differ only in case, or in how a letter is written; rename or take out one first", them.join(" and "))));
            }
        }
    }
    for (p, _) in &c.puts {
        clear(&s.root, p, &t)?;
    }
    for p in &c.removes {
        // A link committed is taken out itself: what it names is not followed.
        no_link(&s.root, if t.is_link(p) { p.rsplit_once('/').map_or(p.as_str(), |(d, _)| d) } else { p })?;
    }
    let mut paths: Vec<String> = c.puts.iter().map(|(p, _)| p.clone()).chain(c.removes.iter().cloned()).collect();
    paths.sort();
    paths.dedup();
    let puts: Vec<(&str, &[u8])> = c.puts.iter().map(|(p, b)| (p.as_str(), b.as_slice())).collect();
    let msg = store::message(subject, &[(&argv, subject)]);
    let commit = s.commit(&paths, &puts, &c.removes, &c.exec, &msg)?;
    if commit.is_empty() {
        anyhow::bail!("git committed nothing of it: is a directory on the way a git repository of its own?");
    }
    // A directory emptied on disk goes too; one a file not committed is
    // in stays.
    for p in &c.removes {
        let mut at = Path::new(p).parent();
        while let Some(d) = at.filter(|d| *d != Path::new(ROOT)) {
            if std::fs::remove_dir(s.root.join(d)).is_err() {
                break;
            }
            at = d.parent();
        }
    }
    Ok(commit)
}

/// `abo corporate add`: each file, by its name and bytes, into `dir`.
pub fn add(s: &mut Store, given: &[(String, Vec<u8>)], dir: &str) -> Result<String> {
    let t = Tree::read(&s.root)?;
    let mut d = names(dir)?;
    spelled(&t, &mut d);
    let mut c = Change::default();
    c.dirs(&t, &d)?;
    let at = join(&d);
    let mut argv = vec!["corporate".into(), "add".into(), "--".into()];
    let given: Vec<(String, &Vec<u8>)> = given.iter().map(|(n, b)| (nfc(n), b)).collect();
    for (i, (name, bytes)) in given.iter().enumerate() {
        if let Some(why) = bad_name(name) {
            return Err(refused(format!("{why}, which no name under corporate/ may: give the file another name first")));
        }
        let p = format!("{at}/{name}");
        if let Some(other) = t.taken(&at, name) {
            return Err(clash(&p, &other));
        }
        if let Some((n, _)) = given[..i].iter().find(|(n, _)| folded(n) == folded(name)) {
            return Err(refused(if n == name { format!("{} is given twice", shown(name)) } else { format!("{} and {} are given, and two names in a directory may not differ only in case, or in how a letter is written", shown(n), shown(name)) }));
        }
        argv.push(p.clone());
        c.puts.push((p, bytes.to_vec()));
    }
    argv.push(at.clone());
    let subject = match given[..] {
        [_] => format!("Add {}", argv[3]),
        _ => format!("Add {} corporate documents to {at}", given.len()),
    };
    commit(s, c, &subject, argv)
}

/// `abo corporate mkdir`.
pub fn mkdir(s: &mut Store, dir: &str) -> Result<String> {
    let t = Tree::read(&s.root)?;
    let mut d = names(dir)?;
    spelled(&t, &mut d);
    let at = join(&d);
    if d.is_empty() || t.is_dir(&at) || t.is_file(&at) {
        return Err(refused(format!("{at} is there already")));
    }
    let mut c = Change::default();
    c.dirs(&t, &d)?;
    commit(s, c, &format!("Make the directory {at}"), vec!["corporate".into(), "mkdir".into(), "--".into(), at])
}

/// `names`, each directory among them that the tree holds written another
/// way, `ä` as `a` and a mark, as the tree writes it.
fn spelled(t: &Tree, names: &mut [String]) {
    for i in 0..names.len() {
        let p = join(&names[..=i]);
        if !t.is_dir(&p) && !t.is_file(&p) && !t.is_link(&p) {
            if let Some(w) = t.written(&p).filter(|w| t.is_dir(w)) {
                names[i] = w.rsplit('/').next().unwrap().to_string();
            }
        }
    }
}

/// What a list of paths is said as in a commit's subject: each, where
/// there are few; how many, where there are more, as `add` says them.
fn said(paths: &[String]) -> String {
    match paths {
        [one] => one.clone(),
        [rest @ .., last] if paths.len() <= 3 => format!("{} and {last}", rest.join(", ")),
        _ => format!("{} paths", paths.len()),
    }
}

/// `abo corporate mv`: each of `from` into `to`, if it is a directory, ends
/// with `/`, or more than one is moved; else the one renamed `to`. One
/// commit, however many.
pub fn mv(s: &mut Store, from: &[String], to: &str) -> Result<String> {
    let t = Tree::read(&s.root)?;
    let srcs = from.iter().map(|f| there(&t, f, "moved")).collect::<Result<Vec<_>>>()?;
    if let Some(keep) = srcs.iter().find(|p| p.rsplit('/').next().is_some_and(|n| folded(n) == folded(KEEP))) {
        let dir = keep.rsplit_once('/').unwrap().0;
        return Err(refused(format!("{keep} keeps its directory, and goes only with it: move {dir} instead")));
    }
    // `to` as typed, where the tree holds it so: of two directories
    // committed by hand, one `ä` and one `a` and a mark, the one typed.
    let raw = to.strip_suffix('/').unwrap_or(to);
    if srcs.iter().any(|s| s == raw) {
        return Err(refused(format!("{raw} is not moved onto itself")));
    }
    let mut d = names(to)?;
    let mut whole = d.clone();
    spelled(&t, &mut whole);
    if raw.starts_with("corporate/") && t.is_dir(raw) {
        whole = raw.split('/').skip(1).map(String::from).collect();
    }
    // `to` is no directory to move into where it is the one moved, written
    // another way: that renames it, as `a` and a mark to `ä`.
    let into = (t.is_dir(&join(&whole)) && !srcs.contains(&join(&whole))) || to.ends_with('/') || srcs.len() > 1;
    if into {
        d = whole;
    } else if let Some((_, parent)) = d.split_last_mut() {
        spelled(&t, parent);
    }
    let given = join(&d);
    for (i, src) in srcs.iter().enumerate() {
        if let Some(other) = srcs.iter().enumerate().find(|(j, o)| *j != i && (src == *o || src.starts_with(&format!("{o}/")))).map(|(_, o)| o) {
            return Err(refused(if src == other { format!("{src} is given twice") } else { format!("{src} is in {other}, which is moved too") }));
        }
    }
    let mut c = Change::default();
    let mut dsts: Vec<(String, &String)> = vec![];
    for src in &srcs {
        let mut d = d.clone();
        if into {
            let name = src.rsplit('/').next().unwrap().to_string();
            if let Some(why) = bad_name(&name) {
                return Err(refused(format!("{why}, which no name under corporate/ may: move it alone, and give it another name")));
            }
            d.push(name);
        }
        let dst = join(&d);
        if dst == *src {
            return Err(refused(if given == *src { format!("{src} is not moved onto itself") } else { format!("{src} is in {given}/ already") }));
        }
        if dst.starts_with(&format!("{src}/")) {
            return Err(refused(format!("{src} is not moved into itself")));
        }
        let (parent, last) = d.split_at(d.len() - 1);
        c.dirs(&t, parent)?;
        if let Some(other) = t.taken_but(&join(parent), &last[0], src) {
            return Err(clash(&dst, &other));
        }
        if let Some((_, other)) = dsts.iter().find(|(o, _)| folded(o) == folded(&dst)) {
            return Err(refused(format!("{other} and {src} would both be {dst}: move them apart")));
        }
        if let Some(link) = t.under(src).into_iter().find(|p| t.is_link(p)) {
            let rm = crate::command::shell(&["corporate", "rm", "--", &link]);
            return Err(refused(format!("{link} is a symbolic link, which abo does not move: `{rm}` takes it out")));
        }
        // Two names carried, `ä` and `a` and a mark, `K` and `k`, would land as one.
        let mut carried: BTreeMap<String, String> = BTreeMap::new();
        for p in t.under(src) {
            let rel: Vec<&str> = p[src.len()..].split('/').filter(|n| !n.is_empty()).collect();
            for i in 0..rel.len() {
                let raw = rel[..=i].join("/");
                if let Some(other) = carried.insert(folded(&raw), raw.clone()).filter(|o| *o != raw) {
                    return Err(refused(format!("{src}/{other} and {src}/{raw} would be one name: rename or take out one first")));
                }
            }
        }
        let under = t.under(src);
        for p in &under {
            no_link(&s.root, p)?;
        }
        // As the last commit holds them, not as a filter on checkout wrote them.
        let blobs = blobs(&s.root, &under)?;
        for (p, bytes) in under.into_iter().zip(blobs) {
            // In NFC, what it carries too: a name committed by hand as `a`
            // and a mark lands as `ä`.
            // The directory it goes into as the tree writes it, never respelled.
            let to = format!("{}/{}", join(parent), nfc(&format!("{}{}", last[0], &p[src.len()..])));
            if t.exec.contains(&p) {
                c.exec.push(to.clone());
            }
            c.puts.push((to, bytes));
            c.removes.push(p);
        }
        dsts.push((dst, src));
    }
    c.gone = srcs.clone();
    let subject = match (&srcs[..], &dsts[..]) {
        ([src], [(dst, _)]) => format!("Move {src} to {dst}"),
        _ => format!("Move {} into {given}", said(&srcs)),
    };
    let to = if to.ends_with('/') { format!("{given}/") } else { given };
    let mut argv: Vec<String> = vec!["corporate".into(), "mv".into(), "--".into()];
    argv.extend(srcs);
    argv.push(to);
    commit(s, c, &subject, argv)
}

/// `abo corporate rm`: each file, or directory with all it holds. One
/// commit, however many.
pub fn rm(s: &mut Store, paths: &[String]) -> Result<String> {
    let t = Tree::read(&s.root)?;
    let mut ats: Vec<String> = vec![];
    for p in paths {
        let at = there(&t, p, "removed")?;
        if let Some((dir, _)) = at.rsplit_once('/').filter(|(_, n)| folded(n) == folded(KEEP)) {
            let rm = crate::command::shell(&["corporate", "rm", "--", dir]);
            return Err(refused(format!("{at} keeps its directory, and goes only with it: `{rm}` takes out the directory")));
        }
        if ats.contains(&at) {
            return Err(refused(format!("{at} is given twice")));
        }
        ats.push(at);
    }
    let mut removes: Vec<String> = ats.iter().flat_map(|a| t.under(a)).collect();
    removes.sort();
    removes.dedup();
    let c = Change { puts: vec![], removes, gone: ats.clone(), exec: vec![] };
    let subject = format!("Remove {}", said(&ats));
    let mut argv: Vec<String> = vec!["corporate".into(), "rm".into(), "--".into()];
    argv.extend(ats);
    commit(s, c, &subject, argv)
}

/// The bytes of each of `paths` as the last commit holds them, read by one
/// `git cat-file --batch`, not one git a file: a directory of thousands
/// moved. Its requests are a line each, so no path may hold a newline,
/// which no name abo gives does.
fn blobs(root: &Path, paths: &[String]) -> Result<Vec<Vec<u8>>> {
    use std::io::Write as _;
    if let Some(p) = paths.iter().find(|p| p.contains('\n')) {
        anyhow::bail!("{}: a name with a newline, which abo does not move: take it out with git", crate::model::answer::printed(p));
    }
    let mut child = std::process::Command::new("git").current_dir(root).args(["cat-file", "--batch"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let asked: String = paths.iter().map(|p| format!("HEAD:{p}\n")).collect();
    let writer = std::thread::spawn(move || stdin.write_all(asked.as_bytes()));
    let out = child.wait_with_output()?;
    writer.join().map_err(|_| anyhow::anyhow!("git cat-file: writing to it failed"))??;
    let (mut at, mut all) = (0, vec![]);
    for p in paths {
        let end = out.stdout[at..].iter().position(|b| *b == b'\n').map(|e| at + e).ok_or_else(|| anyhow::anyhow!("git cat-file: no answer for {p}"))?;
        let head = String::from_utf8_lossy(&out.stdout[at..end]).into_owned();
        let size: usize = match head.split(' ').collect::<Vec<_>>()[..] {
            [_, "blob", n] => n.parse()?,
            _ => anyhow::bail!("git cat-file {p}: {head}"),
        };
        all.push(out.stdout[end + 1..end + 1 + size].to_vec());
        at = end + 1 + size + 1;
    }
    Ok(all)
}
