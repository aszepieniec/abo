//! `abo init` on a terminal, keys pressed as a person presses them. The other
//! tests pipe their answers, and piped input skips the line editor and the VAT
//! method list entirely; these are what to run after bumping rustyline or nix.
//! Each reads what init wrote, not what it drew.
#![cfg(unix)]

use nix::pty::{openpty, Winsize};
use nix::sys::termios::{tcgetattr, LocalFlags};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const LEFT: &str = "\x1b[D";
const HOME: &str = "\x1b[H";
const END: &str = "\x1b[F";
const CTRL_A: &str = "\x01";
const CTRL_C: &str = "\x03";
const CTRL_D: &str = "\x04";
const CTRL_K: &str = "\x0b";

/// What came of one `abo init`, and whether the terminal was left as found.
struct Run {
    status: ExitStatus,
    screen: String,
    books: PathBuf,
    terminal_restored: bool,
}

/// `abo init <name>` in a fresh directory, on a terminal.
fn init(test: &str, name: &str, steps: &[(&str, &str)]) -> Run {
    let base = std::env::temp_dir().join(format!("abo-tty-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let (status, screen, terminal_restored) = abo(&base, &["init", name], steps);
    Run { status, screen, books: base.join(name.to_lowercase()), terminal_restored }
}

/// `abo <args>` in `cwd`, on a terminal: how it ended, everything it drew,
/// and whether it left the terminal as it found it. Each step waits for
/// `prompt` to appear after the previous one and for the screen to settle,
/// as a person would, then presses `keys`. An empty `prompt` only waits to
/// settle: the address's further lines are prompted with spaces, which the
/// editor's redrawing also writes.
fn abo(cwd: &Path, args: &[&str], steps: &[(&str, &str)]) -> (ExitStatus, String, bool) {
    // HOME and XDG_CONFIG_HOME below: somewhere with no git config.
    let base = cwd.ancestors().find(|a| a.file_name().is_some_and(|n| n.to_string_lossy().starts_with("abo-tty-"))).unwrap();
    let size = Winsize { ws_row: 24, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
    let pty = openpty(Some(&size), None).unwrap();
    let before = tcgetattr(&pty.slave).unwrap().local_flags;
    let tty = |fd: &OwnedFd| Stdio::from(fd.try_clone().unwrap());
    let mut child = Command::new(env!("CARGO_BIN_EXE_abo"))
        .args(args)
        .current_dir(cwd)
        .env("TERM", "xterm")
        // No global git identity to offer, and no stray config of the
        // machine running the tests.
        .env("HOME", base)
        .env("XDG_CONFIG_HOME", base)
        .env("GIT_AUTHOR_NAME", "abo test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "abo test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .stdin(tty(&pty.slave))
        .stdout(tty(&pty.slave))
        .stderr(tty(&pty.slave))
        .spawn()
        .unwrap();

    // Everything abo draws. The reader blocks for good once abo is gone,
    // because the slave stays open here to be inspected; it ends with the
    // test binary.
    let screen = Arc::new(Mutex::new(String::new()));
    let master = pty.master;
    {
        let (screen, master) = (screen.clone(), master.try_clone().unwrap());
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = nix::unistd::read(&master, &mut buf) {
                screen.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
    }

    let mut seen = 0;
    for (prompt, keys) in steps {
        let start = Instant::now();
        loop {
            let s = screen.lock().unwrap().clone();
            if let Some(i) = s[seen..].find(prompt) {
                seen += i + prompt.len();
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "no {prompt:?} on screen:\n{s}");
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut drawn = screen.lock().unwrap().len();
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let now = screen.lock().unwrap().len();
            if now == drawn {
                break;
            }
            drawn = now;
        }
        nix::unistd::write(&master, keys.as_bytes()).unwrap();
    }

    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > Duration::from_secs(10) {
            let _ = child.kill();
            panic!("abo {args:?} did not finish:\n{}", screen.lock().unwrap());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    std::thread::sleep(Duration::from_millis(50));
    let after = tcgetattr(&pty.slave).unwrap().local_flags;
    let modes = LocalFlags::ICANON | LocalFlags::ECHO | LocalFlags::ISIG;
    let screen = screen.lock().unwrap().clone();
    (status, screen, after & modes == before & modes)
}

fn config(books: &Path) -> toml::Value {
    toml::from_str(&std::fs::read_to_string(books.join("abo.toml")).unwrap()).unwrap()
}

fn done(r: &Run) -> toml::Value {
    assert!(r.status.success(), "init failed:\n{}", r.screen);
    assert!(r.terminal_restored, "terminal left changed:\n{}", r.screen);
    let c = config(&r.books);
    std::fs::remove_dir_all(r.books.parent().unwrap()).unwrap();
    c
}

fn refused(r: &Run, says: &str) {
    assert!(!r.status.success(), "init went through:\n{}", r.screen);
    assert!(r.screen.contains(says), "no {says:?} on screen:\n{}", r.screen);
    assert!(r.terminal_restored, "terminal left changed:\n{}", r.screen);
    assert!(!r.books.exists(), "{} was made", r.books.display());
    std::fs::remove_dir_all(r.books.parent().unwrap()).unwrap();
}

const ADDRESS: &str = "an empty line ends it:";
const METHOD: &str = "VAT method (up";
const PAYMENT: &str = "Payment option (up";

/// Keys that move and delete within an answer, lines of an address, Ctrl-D
/// on an empty line, and Enter keeping what a question put on the line.
#[test]
fn editing() {
    let r = init(
        "editing",
        "Acme",
        &[
            (ADDRESS, &format!("12 Alrd{LEFT}{LEFT}{LEFT}i\r")),
            ("", &format!("Main StXYZ{LEFT}{LEFT}{LEFT}{CTRL_K}{CTRL_A}1 \r")),
            ("", &format!("art{HOME}E{END}h\r")),
            ("", "\r"),
            ("Country: ", "BE\r"),
            ("Tax registration", CTRL_D),
            (PAYMENT, "s "),
            ("Currency code", "eur\r"),
            (METHOD, &format!("{UP} ")),
            ("Flat rate", "5\r"),
            ("VAT account: ", "\r"),
            ("VAT rate charged", "\r"),
            ("git user.name", "\r"),
            ("git user.email", "\r"),
        ],
    );
    let c = done(&r);
    let (company, vat) = (&c["company"], &c["vat"]);
    assert_eq!(company["address"].as_str(), Some("12 Ailrd\n1 Main St\nEarth"));
    assert_eq!(company["country"].as_str(), Some("BE"));
    assert_eq!(company["tax_id"].as_str(), Some(""));
    assert_eq!(company["currency"].as_str(), Some("EUR"));
    assert_eq!(vat["method"].as_str(), Some("flat"));
    assert_eq!(vat["account"].as_str(), Some("Liabilities:VAT"));
}

/// Up and down, a letter, and space or enter; keys arriving together in one
/// read are each acted on.
#[test]
fn vat_method_list() {
    let blank = |p| (p, "\r");
    for (i, (keys, method)) in [
        (format!("{UP} "), "flat"),
        ("e ".to_string(), "effective"),
        (format!("{DOWN}{DOWN}{UP} "), "effective"),
        (format!("{DOWN}{DOWN}{DOWN}\r"), "none"),
        ("n\r".to_string(), "none"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut steps = vec![
            (ADDRESS, "\r"),
            blank("Country: "),
            blank("Tax registration"),
            (PAYMENT, "s "),
            ("Currency code", "EUR\r"),
            (METHOD, keys.as_str()),
        ];
        match method {
            "flat" => steps.extend([("Flat rate", "5\r"), blank("VAT account"), blank("VAT rate charged")]),
            "effective" => steps.extend([blank("Input VAT"), blank("Output VAT"), blank("VAT rate charged")]),
            _ => {}
        }
        steps.extend([blank("git user.name"), blank("git user.email")]);
        let c = done(&init(&format!("list{i}"), "Acme", &steps));
        assert_eq!(c["vat"]["method"].as_str(), Some(method), "{keys:?}");
    }
}

/// Ctrl-D on a question that needs an answer refuses; Ctrl-C stops, in the
/// line editor and in the list. Nothing is made, and the terminal is put back.
#[test]
fn giving_up() {
    let to_currency = |keys| {
        vec![(ADDRESS, "\r"), ("Country: ", "\r"), ("Tax registration", "\r"), (PAYMENT, "s "), ("Currency code", keys)]
    };
    refused(&init("ctrl-d", "Acme", &to_currency(CTRL_D)), "A currency is needed");
    refused(&init("ctrl-c", "Acme", &to_currency(CTRL_C)), "Error:");
    let mut steps = to_currency("EUR\r");
    steps.push((METHOD, CTRL_C));
    refused(&init("ctrl-c-list", "Acme", &steps), "Error: interrupted");
}

/// Payment options chosen from a list of kinds and asked for part by part:
/// at init, a bank option with a mistyped IBAN asked again, then, asked for,
/// a cash option, then no more; changed later,
/// each part on the line as it stands; and a crypto option added, its
/// undeclared account asked again.
#[test]
fn payment_options() {
    let steps = [
        (ADDRESS, "\r"),
        ("Country: ", "\r"),
        ("Tax registration", "\r"),
        (PAYMENT, " "),
        ("Account holder: ", "\r"),
        ("IBAN (blank if none): ", "CH93 0076 2011 6238 5295 8\r"),
        ("IBAN (blank if none): ", &format!("{CTRL_A}{CTRL_K}ch93 0076 2011 6238 5295 7\r")),
        ("BIC (blank if none): ", "\r"),
        ("name and address", "PostFinance AG\r"),
        ("", "\r"),
        ("Anything else", "\r"),
        ("Name on invoices: ", "\r"),
        ("Id, for abo commands: ", "\r"),
        ("Account the money arrives in: ", "\r"),
        ("Add another payment option [no]: ", "y\r"),
        (PAYMENT, &format!("{DOWN} ")),
        ("Anything else", "At our office\r"),
        ("", "\r"),
        ("Name on invoices: ", "\r"),
        ("Id, for abo commands: ", "\r"),
        ("Account the money arrives in: ", "\r"),
        ("Add another payment option [no]: ", "\r"),
        ("Currency code", "EUR\r"),
        (METHOD, "n "),
        ("git user.name", "\r"),
        ("git user.email", "\r"),
    ];
    let r = init("payment", "Acme", &steps);
    assert!(r.status.success(), "init failed:\n{}", r.screen);
    assert!(r.screen.contains("check digits do not match"), "{}", r.screen);

    let (status, screen, restored) = abo(
        &r.books,
        &["payment-options", "edit", "bank"],
        &[
            (PAYMENT, " "),
            ("Account holder: ", &format!("{END} GmbH\r")),
            ("IBAN (blank if none): ", "\r"),
            ("BIC (blank if none): ", "pofichbexxx\r"),
            ("name and address", "\r"),
            ("", "3030 Bern\r"),
            ("", "\r"),
            ("Anything else", "\r"),
            ("Name on invoices: ", &format!("{CTRL_A}{CTRL_K}PostFinance\r")),
            ("Id, for abo commands: ", &format!("{CTRL_A}{CTRL_K}postfinance\r")),
            ("Account the money arrives in: ", "\r"),
        ],
    );
    assert!(status.success() && restored, "edit failed:\n{screen}");

    let (status, screen, restored) = abo(
        &r.books,
        &["payment-options", "add"],
        &[
            (PAYMENT, &format!("{DOWN}{DOWN} ")),
            ("Network (such as", "Bitcoin\r"),
            ("Wallet address: ", "bc1qexample\r"),
            ("Anything else", "\r"),
            ("Name on invoices: ", "\r"),
            ("Id, for abo commands: ", "\r"),
            // Not declared yet, it is declared with the option.
            ("Account the money arrives in: ", "\r"),
        ],
    );
    assert!(status.success() && restored, "add failed:\n{screen}");

    let c = done(&r);
    // A table of options, keyed by id.
    let (bank, cash, wallet) = (&c["payment"]["postfinance"], &c["payment"]["cash"], &c["payment"]["crypto"]);
    assert_eq!(cash["label"].as_str(), Some("Cash"));
    assert_eq!(cash["details"].as_str(), Some("At our office"));
    assert_eq!(cash["account"].as_str(), Some("Assets:Cash"));
    assert_eq!(bank["kind"].as_str(), Some("bank"));
    assert_eq!(bank["label"].as_str(), Some("PostFinance"));
    assert_eq!(bank["holder"].as_str(), Some("Acme GmbH"));
    assert_eq!(bank["iban"].as_str(), Some("CH93 0076 2011 6238 5295 7"));
    assert_eq!(bank["bic"].as_str(), Some("POFICHBEXXX"));
    assert_eq!(bank["bank"].as_str(), Some("PostFinance AG\n3030 Bern"));
    assert_eq!(bank["account"].as_str(), Some("Assets:Bank"));
    assert_eq!(wallet["label"].as_str(), Some("Bitcoin"));
    assert_eq!(wallet["network"].as_str(), Some("Bitcoin"));
    assert_eq!(wallet["address"].as_str(), Some("bc1qexample"));
    assert_eq!(wallet["account"].as_str(), Some("Assets:Crypto"));
}
