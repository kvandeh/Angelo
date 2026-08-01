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
    Untestable,
}

/// Every status, so `parse` and the round-trip test read from one list.
pub const STATUSES: &[Status] = &[
    Status::Killed,
    Status::Survived,
    Status::Timeout,
    Status::Error,
    Status::Untestable,
];

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Killed => "killed",
            Status::Survived => "survived",
            Status::Timeout => "timeout",
            Status::Error => "error",
            Status::Untestable => "untestable",
        }
    }

    /// None for anything else in the DB, which means `pending`.
    pub fn parse(text: &str) -> Option<Status> {
        STATUSES
            .iter()
            .copied()
            .find(|status| status.as_str() == text)
    }

    /// A timeout counts as detected: the mutant observably changed behaviour.
    pub fn is_detected(self) -> bool {
        matches!(self, Status::Killed | Status::Timeout)
    }

    /// This verdict's name in the mutation-testing-report schema.
    ///
    /// `executed` is what splits `survived` in two. A mutant no test ever ran
    /// is `NoCoverage`, and that is a different finding with a different fix
    /// from a mutant a test did run and failed to notice.
    ///
    /// `error` maps to `RuntimeError` rather than `CompileError` on purpose. A
    /// broken test command produces a run of nothing but errors, and it has to
    /// stay visible in the report rather than reading as a clean bill of
    /// health.
    pub fn schema_name(self, executed: bool) -> &'static str {
        match self {
            Status::Killed => "Killed",
            Status::Timeout => "Timeout",
            Status::Survived if executed => "Survived",
            Status::Survived => "NoCoverage",
            Status::Error => "RuntimeError",
            Status::Untestable => "Ignored",
        }
    }
}

impl Mutant {
    /// Splice this mutant into the source, in place. A batch is applied back to
    /// front so that earlier byte offsets stay valid, and the caller has already
    /// proved the range still holds `original`, so the bounds are char
    /// boundaries.
    pub fn splice_into(&self, source: &mut String) {
        source.replace_range(self.byte_start..self.byte_end, &self.replacement);
    }

    /// The file path with forward slashes, as coverage.py records it.
    pub fn coverage_file(&self) -> String {
        self.file.to_string_lossy().replace('\\', "/")
    }

    /// How this file is named in a report file: relative to the project root,
    /// forward slashes, no `./`.
    ///
    /// A report is read by tools that resolve these back to real files, and a
    /// Windows backslash resolves to nothing on the machine reading it. The
    /// `./` goes for the same reason, since `./src/x.py` and `src/x.py` are one
    /// file to a person and two strings to a consumer.
    pub fn report_path(&self) -> String {
        let forward = self.coverage_file();
        forward.strip_prefix("./").unwrap_or(&forward).to_string()
    }

    /// Which family of fault this mutant plants, named in the vocabulary the
    /// mutation-testing-report schema's viewers already speak.
    ///
    /// Derived from the token rather than stored: `CREATE TABLE IF NOT EXISTS`
    /// cannot add a column to a `.angelo/` an older build wrote, and a
    /// migration is not worth one string. The arms have to keep up with
    /// `operators!`, which is what `every_operator_has_a_family` checks.
    pub fn mutator(&self) -> &'static str {
        match self.original.as_str() {
            "+" | "-" | "*" | "/" | "//" | "%" | "**" => "ArithmeticOperator",
            "&" | "|" | "^" | "<<" | ">>" => "BitwiseOperator",
            "==" | "!=" | "<" | "<=" | ">" | ">=" => "EqualityOperator",
            "and" | "or" => "LogicalOperator",
            "True" | "False" => "BooleanLiteral",
            "not" | "~" => "UnaryOperator",
            "is" | "is not" | "in" | "not in" => "ConditionalExpression",
            "break" | "continue" | "return" => "StatementSwap",
            // Every comparison ending in `=` was matched above, so what is left
            // is an augmented assignment.
            text if text.ends_with('=') => "AssignmentOperator",
            // A prefix puts `f`, `b` or `r` before the quote, so the closing one
            // is the reliable end to look at.
            text if text.ends_with(['"', '\'']) => "StringLiteral",
            text if text.starts_with(|c: char| c.is_ascii_digit()) => "NumberLiteral",
            // All that reaches here is `name_swaps`: a string method or deepcopy.
            _ => "MethodExpression",
        }
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
/// an attribute, `x.lower()` is a method call, a bare `lower` is not.
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
        // escapes must not be touched, a lone backslash would break syntax.
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

pub(crate) fn enumerate_source(source: &str, file: &Path) -> Result<Vec<Mutant>> {
    let parsed = ruff_python_parser::parse_module(source)
        .with_context(|| format!("parsing {}", file.display()))?;

    let mut mutants = Vec::new();
    let mut previous: Option<TokenKind> = None;
    let mut loop_headers = ForHeaders::default();
    let mut lines = Lines::default();

    for token in parsed.tokens().iter() {
        let kind = token.kind();
        let start = token.range().start().to_usize();
        let end = token.range().end().to_usize();
        let text = &source[start..end];

        if loop_headers.skips(kind) {
            previous = Some(kind);
            continue;
        }
        let line = lines.at(source, start);
        for replacement in replacements(kind, text, previous == Some(TokenKind::Dot)) {
            if replacement == text {
                continue;
            }
            mutants.push(Mutant {
                id: 0,
                file: file.to_path_buf(),
                line,
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

/// A place in a file, 1-based on both axes, with the column counted in
/// **characters**. The mutation-testing-report schema requires exactly this,
/// and a byte column would drift on any line holding a non-ASCII string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

/// Line numbers for offsets that only ever move forward. The token stream is
/// already in order, so counting from where the last question left off scans a
/// file once, where counting from byte zero scanned it once per mutant.
#[derive(Default)]
pub struct Lines {
    scanned: usize,
    newlines: u32,
    /// Just past the most recent newline: where the current line's columns are
    /// counted from.
    line_start: usize,
}

impl Lines {
    /// Move the cursor up to `byte_offset`, taking in what it passes over.
    ///
    /// An offset behind the cursor cannot happen from a sorted token stream,
    /// and asking for one repeats the last answer rather than panicking.
    fn advance(&mut self, source: &str, byte_offset: usize) {
        let span = source
            .as_bytes()
            .get(self.scanned..byte_offset)
            .unwrap_or_default();
        // Two passes rather than one indexed loop: both of these vectorise, and
        // `advance` runs once per token across every file in the project.
        self.newlines += span.iter().filter(|byte| **byte == b'\n').count() as u32;
        if let Some(last) = span.iter().rposition(|byte| *byte == b'\n') {
            self.line_start = self.scanned + last + 1;
        }
        self.scanned = byte_offset;
    }

    pub fn at(&mut self, source: &str, byte_offset: usize) -> u32 {
        self.advance(source, byte_offset);
        self.newlines + 1
    }

    pub fn position(&mut self, source: &str, byte_offset: usize) -> Position {
        self.advance(source, byte_offset);
        Position {
            line: self.newlines + 1,
            column: source
                .get(self.line_start..byte_offset)
                .unwrap_or_default()
                .chars()
                .count() as u32
                + 1,
        }
    }
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
    fn splicing_writes_the_replacement_in_place() {
        let mut source = "x = 1 < 2\n".to_string();
        let mutant = mutants(&source)
            .into_iter()
            .find(|m| m.original == "<")
            .unwrap();
        mutant.splice_into(&mut source);
        assert_eq!(source, "x = 1 <= 2\n");
    }

    #[test]
    fn line_numbers_start_at_one() {
        let found = mutants("a = 1\nb = 2 + 3\n");
        assert!(found.iter().all(|m| m.line >= 1));
        assert_eq!(found.iter().find(|m| m.original == "+").unwrap().line, 2);
    }

    /// The counter carries a cursor, so it has to answer what a scan from byte
    /// zero would: the same line twice for two mutants on one token, and the
    /// right one after a jump over several newlines.
    #[test]
    fn line_numbers_pick_up_where_the_last_answer_left_off() {
        let source = "a = 1\nb = 2\n\n\nc = 3\n";
        let mut lines = Lines::default();
        assert_eq!(lines.at(source, 0), 1);
        assert_eq!(lines.at(source, 4), 1);
        assert_eq!(lines.at(source, 4), 1);
        assert_eq!(lines.at(source, 6), 2);
        assert_eq!(lines.at(source, 14), 5);
        assert_eq!(lines.at(source, source.len()), 6);
    }

    /// A multi-byte character is several bytes and no newline, so a line number
    /// counts it once for its file rather than once for its bytes.
    #[test]
    fn line_numbers_survive_multibyte_characters() {
        let found = mutants("s = 'héllo'\nn = 1 + 2\n");
        assert_eq!(found.iter().find(|m| m.original == "+").unwrap().line, 2);
    }

    #[test]
    fn statuses_survive_a_round_trip() {
        for status in STATUSES {
            assert_eq!(Status::parse(status.as_str()), Some(*status));
        }
        assert_eq!(Status::parse("pending"), None);
        assert!(Status::Timeout.is_detected());
        assert!(!Status::Survived.is_detected());
        // A mutant nobody could fairly try is not a mutant nobody detected.
        assert!(!Status::Untestable.is_detected());
    }

    #[test]
    fn coverage_file_uses_forward_slashes() {
        let found = enumerate_source("a = 1 + 2\n", Path::new("src\\pkg\\mod.py")).unwrap();
        assert_eq!(found[0].coverage_file(), "src/pkg/mod.py");
    }

    /// A report path has to resolve on the machine reading the report, which is
    /// not always the machine that wrote it.
    #[test]
    fn a_report_path_is_relative_and_forward_slashed() {
        let windows = enumerate_source("a = 1 + 2\n", Path::new(".\\src\\pkg\\mod.py")).unwrap();
        assert_eq!(windows[0].report_path(), "src/pkg/mod.py");
        let plain = enumerate_source("a = 1 + 2\n", Path::new("./calc.py")).unwrap();
        assert_eq!(plain[0].report_path(), "calc.py");
        let bare = enumerate_source("a = 1 + 2\n", Path::new("calc.py")).unwrap();
        assert_eq!(bare[0].report_path(), "calc.py");
    }

    #[test]
    fn a_removal_reads_clearly_in_the_report() {
        let mutant = mutants("if not ready:\n    pass\n")
            .into_iter()
            .find(|m| m.original == "not")
            .unwrap();
        assert!(mutant.to_string().contains("<removed>"));
    }

    /// Both axes are 1-based, and the column restarts at each newline. The
    /// report schema sets `minimum: 1` on both, so a 0 here is not merely off
    /// by one, it is invalid.
    #[test]
    fn positions_count_from_one_on_both_axes() {
        let source = "a = 1\nbb = 22\n";
        let mut lines = Lines::default();
        assert_eq!(lines.position(source, 0), Position { line: 1, column: 1 });
        assert_eq!(lines.position(source, 4), Position { line: 1, column: 5 });
        assert_eq!(lines.position(source, 6), Position { line: 2, column: 1 });
        assert_eq!(lines.position(source, 8), Position { line: 2, column: 3 });
    }

    /// A column is a count of characters, not of bytes. `é` is two bytes, so a
    /// byte column would report the `+` one place further right than it is.
    #[test]
    fn columns_count_characters_rather_than_bytes() {
        let source = "s = 'héllo' + x\n";
        let plus = source.find('+').expect("the operator");
        let mut lines = Lines::default();
        assert_eq!(
            lines.position(source, plus),
            Position {
                line: 1,
                column: 13
            }
        );
    }

    /// Two blank lines in a row leave `line_start` on the last of them, so a
    /// column after a run of newlines still restarts.
    #[test]
    fn a_run_of_newlines_still_restarts_the_column() {
        let source = "a = 1\n\n\n    b = 2\n";
        let mut lines = Lines::default();
        assert_eq!(lines.position(source, 12), Position { line: 4, column: 5 });
    }

    /// A mutant no test ran is a different finding from one a test ran and
    /// missed, and the schema has a separate name for it.
    #[test]
    fn the_schema_splits_survived_on_whether_anything_ran() {
        assert_eq!(Status::Survived.schema_name(true), "Survived");
        assert_eq!(Status::Survived.schema_name(false), "NoCoverage");
        assert_eq!(Status::Killed.schema_name(true), "Killed");
        assert_eq!(Status::Timeout.schema_name(true), "Timeout");
        assert_eq!(Status::Untestable.schema_name(false), "Ignored");
    }

    /// A run of nothing but errors is what a broken test command looks like.
    /// `CompileError` would let a consumer treat it as noise to filter out.
    #[test]
    fn an_errored_mutant_stays_visible_in_the_schema() {
        assert_eq!(Status::Error.schema_name(true), "RuntimeError");
    }

    /// The mutator name carries the whole message a report reader sees, so
    /// every operator needs one. Each replacement in the table is also some
    /// other operator's original, so the table doubles as the input list: add a
    /// token whose family `mutator` does not know and this fails.
    #[test]
    fn every_operator_has_a_family() {
        for (_, replacement) in MUTABLE_TOKENS {
            let mutant = Mutant {
                id: 0,
                file: PathBuf::new(),
                line: 1,
                byte_start: 0,
                byte_end: 0,
                original: replacement.to_string(),
                replacement: String::new(),
            };
            assert_ne!(
                mutant.mutator(),
                "MethodExpression",
                "{replacement:?} fell through to the catch-all"
            );
        }
    }

    #[test]
    fn families_name_what_the_mutant_actually_did() {
        let family = |source: &str, original: &str| {
            mutants(source)
                .into_iter()
                .find(|m| m.original == original)
                .unwrap_or_else(|| panic!("no mutant on {original:?}"))
                .mutator()
        };
        assert_eq!(family("a = 1 + 2\n", "+"), "ArithmeticOperator");
        assert_eq!(family("a = b << 2\n", "<<"), "BitwiseOperator");
        assert_eq!(family("a = b >= 2\n", ">="), "EqualityOperator");
        assert_eq!(family("a = b and c\n", "and"), "LogicalOperator");
        assert_eq!(family("a = True\n", "True"), "BooleanLiteral");
        assert_eq!(family("a = 1\n", "1"), "NumberLiteral");
        assert_eq!(family("a = 'hi'\n", "'hi'"), "StringLiteral");
        assert_eq!(family("if not a:\n    pass\n", "not"), "UnaryOperator");
        assert_eq!(family("a += 1\n", "+="), "AssignmentOperator");
        assert_eq!(family("a = b.lower()\n", "lower"), "MethodExpression");
        assert_eq!(family("a = b is c\n", "is"), "ConditionalExpression");
    }

    /// A prefixed or raw string does not start with a quote, so the family has
    /// to look at the end of the token rather than the beginning.
    #[test]
    fn a_prefixed_string_is_still_a_string() {
        let mutant = |original: &str| Mutant {
            id: 0,
            file: PathBuf::new(),
            line: 1,
            byte_start: 0,
            byte_end: 0,
            original: original.to_string(),
            replacement: String::new(),
        };
        assert_eq!(mutant("f'{x}'").mutator(), "StringLiteral");
        assert_eq!(mutant("rb\"raw\"").mutator(), "StringLiteral");
    }
}
