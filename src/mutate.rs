use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ruff_python_ast::token::TokenKind;
use ruff_text_size::Ranged;

pub struct Mutant {
    /// 0 until the database assigns one.
    pub id: i64,
    pub file: PathBuf,
    pub line: u32,
    pub byte_start: usize,
    pub byte_end: usize,
    pub original: String,
    pub replacement: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Killed,
    Survived,
    Timeout,
    Error,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Killed => "killed",
            Status::Survived => "survived",
            Status::Timeout => "timeout",
            Status::Error => "error",
        }
    }

    /// None for anything else in the DB, which means `pending`.
    pub fn parse(text: &str) -> Option<Status> {
        [
            Status::Killed,
            Status::Survived,
            Status::Timeout,
            Status::Error,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
    }

    /// A timeout counts as detected: the mutant observably changed behaviour.
    pub fn is_detected(self) -> bool {
        matches!(self, Status::Killed | Status::Timeout)
    }
}

impl Mutant {
    pub fn apply(&self, source: &str) -> String {
        format!(
            "{}{}{}",
            &source[..self.byte_start],
            self.replacement,
            &source[self.byte_end..]
        )
    }

    /// The file path with forward slashes, as coverage.py records it.
    pub fn coverage_file(&self) -> String {
        self.file.to_string_lossy().replace('\\', "/")
    }
}

impl fmt::Display for Mutant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let shown = |text: &str| {
            if text.is_empty() {
                "<removed>".to_string()
            } else if text.chars().count() > 24 {
                format!("{}…", text.chars().take(24).collect::<String>())
            } else {
                text.to_string()
            }
        };
        write!(
            f,
            "{}:{} {} -> {}",
            self.file.display(),
            self.line,
            shown(&self.original),
            shown(&self.replacement)
        )
    }
}

/// One token in, one replacement out. Mirrors mutmut's `_operator_mapping`
/// plus its keyword swaps, so the two tools plant comparable faults.
/// Written as a macro so the table stays a table and its test list is
/// generated from the same lines.
macro_rules! operators {
    ($($token:ident => $replacement:literal),+ $(,)?) => {
        fn swapped(kind: TokenKind) -> Option<&'static str> {
            match kind {
                $(TokenKind::$token => Some($replacement),)+
                _ => None,
            }
        }

        #[cfg(test)]
        const MUTABLE_TOKENS: &[(TokenKind, &str)] =
            &[$((TokenKind::$token, $replacement)),+];
    };
}

operators! {
    // arithmetic
    Plus => "-",
    Minus => "+",
    Star => "/",
    Slash => "*",
    DoubleSlash => "/",
    Percent => "/",
    DoubleStar => "*",
    // bitwise and shifts
    Amper => "|",
    Vbar => "&",
    CircumFlex => "&",
    LeftShift => ">>",
    RightShift => "<<",
    // comparison
    EqEqual => "!=",
    NotEqual => "==",
    Less => "<=",
    LessEqual => "<",
    Greater => ">=",
    GreaterEqual => ">",
    // boolean and keyword swaps
    And => "or",
    Or => "and",
    True => "False",
    False => "True",
    Break => "return",
    Continue => "break",
    Is => "is not",
    // augmented assignment
    PlusEqual => "-=",
    MinusEqual => "+=",
    StarEqual => "/=",
    SlashEqual => "*=",
    DoubleSlashEqual => "/=",
    PercentEqual => "/=",
    DoubleStarEqual => "*=",
    AmperEqual => "|=",
    VbarEqual => "&=",
    CircumflexEqual => "&=",
    LeftShiftEqual => ">>=",
    RightShiftEqual => "<<=",
}

/// Symmetric string methods, from mutmut's list. Only swapped when the name is
/// an attribute — `x.lower()` is a method call, a bare `lower` is not.
const METHOD_SWAPS: &[(&str, &str)] = &[
    ("lower", "upper"),
    ("upper", "lower"),
    ("lstrip", "rstrip"),
    ("rstrip", "lstrip"),
    ("find", "rfind"),
    ("rfind", "find"),
    ("ljust", "rjust"),
    ("rjust", "ljust"),
    ("index", "rindex"),
    ("rindex", "index"),
    ("removeprefix", "removesuffix"),
    ("removesuffix", "removeprefix"),
    ("partition", "rpartition"),
    ("rpartition", "partition"),
    ("split", "rsplit"),
    ("rsplit", "split"),
];

/// Every replacement a token admits. Most give one; strings give up to three.
fn replacements(kind: TokenKind, text: &str, after_dot: bool) -> Vec<String> {
    if let Some(swap) = swapped(kind) {
        return vec![swap.to_string()];
    }
    match kind {
        // `not x` -> `x`, `~x` -> `x`. Also turns `is not` into `is` and
        // `not in` into `in`, which is mutmut's IsNot/NotIn swap.
        TokenKind::Not | TokenKind::Tilde => vec![String::new()],
        TokenKind::Int | TokenKind::Float => bumped_number(text).into_iter().collect(),
        TokenKind::String => string_variants(text),
        TokenKind::Name => name_swaps(text, after_dot),
        _ => Vec::new(),
    }
}

/// mutmut's number mutation: whatever it is, make it one bigger.
fn bumped_number(text: &str) -> Option<String> {
    let clean = text.replace('_', "");
    let radix = match clean.get(..2).map(str::to_ascii_lowercase).as_deref() {
        Some("0x") => Some(16),
        Some("0o") => Some(8),
        Some("0b") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let value = i128::from_str_radix(&clean[2..], radix).ok()?;
        return Some((value + 1).to_string());
    }
    if let Ok(value) = clean.parse::<i128>() {
        return Some((value + 1).to_string());
    }
    let value = clean.parse::<f64>().ok()?;
    Some((value + 1.0).to_string())
}

/// mutmut wraps string contents in XX markers and flips their case. Triple
/// quoted strings are skipped: they are nearly always docs.
fn string_variants(text: &str) -> Vec<String> {
    if text.contains("\"\"\"") || text.contains("'''") {
        return Vec::new();
    }
    let Some(open) = text.find(['"', '\'']) else {
        return Vec::new();
    };
    let quote = &text[open..open + 1];
    let prefix = &text[..open];
    let Some(inner) = text
        .get(open + 1..text.len().saturating_sub(1))
        .filter(|_| text.ends_with(quote))
    else {
        return Vec::new();
    };

    let mut variants = vec![format!("{prefix}{quote}XX{inner}XX{quote}")];
    for cased in [inner.to_uppercase(), inner.to_lowercase()] {
        // Case flips only mean something when they change the string, and
        // escapes must not be touched — a lone backslash would break syntax.
        if cased != inner && !inner.contains('\\') {
            variants.push(format!("{prefix}{quote}{cased}{quote}"));
        }
    }
    variants
}

fn name_swaps(text: &str, after_dot: bool) -> Vec<String> {
    if text == "deepcopy" {
        return vec!["copy".to_string()];
    }
    if !after_dot {
        return Vec::new();
    }
    METHOD_SWAPS
        .iter()
        .find(|(from, _)| *from == text)
        .map(|(_, to)| vec![to.to_string()])
        .into_iter()
        .flatten()
        .collect()
}

pub fn enumerate_file(file: &Path) -> Result<Vec<Mutant>> {
    let source = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    enumerate_source(&source, file)
}

fn enumerate_source(source: &str, file: &Path) -> Result<Vec<Mutant>> {
    let parsed = ruff_python_parser::parse_module(source)
        .with_context(|| format!("parsing {}", file.display()))?;

    let mut mutants = Vec::new();
    let mut previous: Option<TokenKind> = None;
    let mut loop_headers = ForHeaders::default();

    for token in parsed.tokens().iter() {
        let kind = token.kind();
        let start = token.range().start().to_usize();
        let end = token.range().end().to_usize();
        let text = &source[start..end];

        if loop_headers.skips(kind) {
            previous = Some(kind);
            continue;
        }
        for replacement in replacements(kind, text, previous == Some(TokenKind::Dot)) {
            if replacement == text {
                continue;
            }
            mutants.push(Mutant {
                id: 0,
                file: file.to_path_buf(),
                line: line_of(source, start),
                byte_start: start,
                byte_end: end,
                original: text.to_string(),
                replacement,
            });
        }
        previous = Some(kind);
    }
    Ok(mutants)
}

/// `for x in y` is not a membership test. Swapping that `in` produces a syntax
/// error every time, so the loop's own `in` is left alone.
#[derive(Default)]
struct ForHeaders {
    open_loops: usize,
}

impl ForHeaders {
    fn skips(&mut self, kind: TokenKind) -> bool {
        match kind {
            TokenKind::For => {
                self.open_loops += 1;
                false
            }
            TokenKind::In if self.open_loops > 0 => {
                self.open_loops -= 1;
                true
            }
            _ => false,
        }
    }
}

fn line_of(source: &str, byte_offset: usize) -> u32 {
    source[..byte_offset]
        .bytes()
        .filter(|b| *b == b'\n')
        .count() as u32
        + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mutants(source: &str) -> Vec<Mutant> {
        enumerate_source(source, Path::new("x.py")).unwrap()
    }

    fn changes(source: &str) -> Vec<(String, String)> {
        mutants(source)
            .into_iter()
            .map(|m| (m.original, m.replacement))
            .collect()
    }

    #[test]
    fn every_listed_token_produces_its_replacement() {
        for (kind, replacement) in MUTABLE_TOKENS {
            assert_eq!(swapped(*kind), Some(*replacement), "{kind:?}");
        }
        assert_eq!(swapped(TokenKind::Name), None);
    }

    #[test]
    fn swaps_arithmetic_and_bumps_numbers() {
        let found = changes("a = 1 + 2 * 3\n");
        assert!(found.contains(&("+".into(), "-".into())));
        assert!(found.contains(&("*".into(), "/".into())));
        assert!(found.contains(&("1".into(), "2".into())));
        assert!(found.contains(&("3".into(), "4".into())));
    }

    #[test]
    fn swaps_augmented_assignment_and_bitwise() {
        let found = changes("x //= 2\ny ^= 3\nz <<= 4\n");
        assert!(found.contains(&("//=".into(), "/=".into())));
        assert!(found.contains(&("^=".into(), "&=".into())));
        assert!(found.contains(&("<<=".into(), ">>=".into())));
    }

    #[test]
    fn bumps_numbers_in_any_base() {
        assert_eq!(bumped_number("41"), Some("42".into()));
        assert_eq!(bumped_number("1_000"), Some("1001".into()));
        assert_eq!(bumped_number("0x0f"), Some("16".into()));
        assert_eq!(bumped_number("0b101"), Some("6".into()));
        assert_eq!(bumped_number("0o17"), Some("16".into()));
        assert_eq!(bumped_number("1.5"), Some("2.5".into()));
    }

    #[test]
    fn wraps_strings_and_flips_their_case() {
        let variants = string_variants("\"abc\"");
        assert!(variants.contains(&"\"XXabcXX\"".to_string()));
        assert!(variants.contains(&"\"ABC\"".to_string()));
        // A docstring is not a fault worth planting.
        assert!(string_variants("\"\"\"docs\"\"\"").is_empty());
        // No letters means no case variant, only the XX marker.
        assert_eq!(string_variants("\"123\"").len(), 1);
    }

    #[test]
    fn removes_unary_not_and_invert() {
        let found = changes("if not ready:\n    pass\n");
        assert!(found.contains(&("not".into(), String::new())));
        assert!(changes("x = ~y\n").contains(&("~".into(), String::new())));
    }

    #[test]
    fn swaps_string_methods_only_on_attributes() {
        assert!(changes("s.lower()\n").contains(&("lower".into(), "upper".into())));
        // A local variable that happens to be called `lower` is not a method.
        assert!(!changes("lower = 1\n").iter().any(|(o, _)| o == "lower"));
    }

    #[test]
    fn leaves_the_in_of_a_for_loop_alone() {
        assert!(
            !changes("for x in items:\n    pass\n")
                .iter()
                .any(|(o, _)| o == "in")
        );
        // A real membership test is still fair game via `not`.
        assert!(
            changes("if x not in items:\n    pass\n")
                .iter()
                .any(|(o, _)| o == "not")
        );
    }

    #[test]
    fn leaves_strings_inside_code_alone_but_mutates_the_literal() {
        let found = changes("s = 'a + b'\n");
        assert!(found.iter().all(|(original, _)| original == "'a + b'"));
    }

    #[test]
    fn apply_splices_the_replacement() {
        let source = "x = 1 < 2\n";
        let mutant = mutants(source)
            .into_iter()
            .find(|m| m.original == "<")
            .unwrap();
        assert_eq!(mutant.apply(source), "x = 1 <= 2\n");
    }

    #[test]
    fn line_numbers_start_at_one() {
        let found = mutants("a = 1\nb = 2 + 3\n");
        assert!(found.iter().all(|m| m.line >= 1));
        assert_eq!(found.iter().find(|m| m.original == "+").unwrap().line, 2);
    }

    #[test]
    fn statuses_survive_a_round_trip() {
        for status in [
            Status::Killed,
            Status::Survived,
            Status::Timeout,
            Status::Error,
        ] {
            assert_eq!(Status::parse(status.as_str()), Some(status));
        }
        assert_eq!(Status::parse("pending"), None);
        assert!(Status::Timeout.is_detected());
        assert!(!Status::Survived.is_detected());
    }

    #[test]
    fn coverage_file_uses_forward_slashes() {
        let found = enumerate_source("a = 1 + 2\n", Path::new("src\\pkg\\mod.py")).unwrap();
        assert_eq!(found[0].coverage_file(), "src/pkg/mod.py");
    }

    #[test]
    fn a_removal_reads_clearly_in_the_report() {
        let mutant = mutants("if not ready:\n    pass\n")
            .into_iter()
            .find(|m| m.original == "not")
            .unwrap();
        assert!(mutant.to_string().contains("<removed>"));
    }
}
