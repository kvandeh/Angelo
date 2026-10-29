use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ruff_python_ast::token::TokenKind;
use ruff_python_ast::{ExceptHandler, Expr, Stmt};
use ruff_text_size::{Ranged, TextRange};

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

    /// The SonarQube rule id this verdict raises, if it raises one.
    ///
    /// Only survivors are findings a developer can act on in their own code. A
    /// `killed` mutant is the suite working; an `error` or `untestable` one is
    /// Angelo reporting on itself, and a broken splice is not a smell in
    /// somebody's file. **That is why a run has to be gated on `--fail-under`
    /// and not on this**: a run where everything errored raises nothing at all,
    /// and an empty issue list reads as a clean bill of health.
    pub fn sonar_rule(self, executed: bool) -> Option<&'static str> {
        match self {
            Status::Survived if executed => Some(SURVIVED_RULE),
            Status::Survived => Some(NO_COVERAGE_RULE),
            Status::Killed | Status::Timeout | Status::Error | Status::Untestable => None,
        }
    }
}

/// A test runs the line and asserts nothing about it.
pub const SURVIVED_RULE: &str = "MutantSurvived";
/// No test runs the line at all.
pub const NO_COVERAGE_RULE: &str = "MutantNoCoverage";

/// Every family `Mutant::mutator` can name. This is also the vocabulary the
/// `operators` config key speaks, so a run cannot enable a family that does not
/// exist, and `every_family_is_listed` fails if `mutator` learns a name this
/// list does not have.
pub const FAMILIES: &[&str] = &[
    "ArithmeticOperator",
    "AssignmentOperator",
    "BitwiseOperator",
    "BlockStatement",
    "BooleanLiteral",
    "ConditionalExpression",
    "EqualityOperator",
    "LogicalOperator",
    "MethodExpression",
    "NumberLiteral",
    "StatementSwap",
    "StringLiteral",
    "UnaryOperator",
];

/// The families a run plants unless the config says otherwise.
///
/// Three are left out, and the evidence for leaving them out is stronger than
/// the evidence that put them in. `UnaryOperator` is unary operator insertion,
/// which of the five operators Google measures across 16.9 million mutants has
/// both the lowest survival rate (9.6% against 12.5% overall) and the lowest
/// developer-judged productivity (74.5%): its mutants are the most likely to be
/// killed by tests that already exist, and the least likely to be worth acting
/// on when they survive. `NumberLiteral` and `StringLiteral` are constant
/// replacement, which a regression search over 108 operators declined to select
/// because it generates large numbers of near-identical mutants, and which
/// cannot reproduce a real literal fault anyway: those need one specific wrong
/// value, and an operator has no way to guess which.
///
/// Turning one back on is a line in angelo.conf, and a project with evidence
/// that its own faults live in its literals should.
pub const DEFAULT_FAMILIES: &[&str] = &[
    "ArithmeticOperator",
    "AssignmentOperator",
    "BitwiseOperator",
    "BlockStatement",
    "BooleanLiteral",
    "ConditionalExpression",
    "EqualityOperator",
    "LogicalOperator",
    "MethodExpression",
    "StatementSwap",
];

/// Calls that exist for a person to read rather than for the program to compute
/// with. Nothing inside one is worth mutating, whatever the operator.
///
/// Kept short and kept to names that are almost never anything else. `debug`
/// is absent for that reason: plenty of projects have a `debug` flag, and an
/// arid list that turns away real code raises the score silently, which is the
/// one failure this tool must not have.
pub const DEFAULT_ARID: &[&str] = &["log", "logger", "logging", "print", "warn", "warnings"];

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
        // A whole statement replaced by `pass` is a deletion, and no token
        // operator in the table produces `pass`, so the replacement settles it
        // alone. It has to be asked first: a deleted `break` is a deletion, not
        // the `break` -> `return` swap its original would otherwise match.
        if self.replacement == "pass" {
            return "BlockStatement";
        }
        match self.original.as_str() {
            "+" | "-" | "*" | "/" | "//" | "%" | "**" => "ArithmeticOperator",
            "&" | "|" | "^" | "<<" | ">>" => "BitwiseOperator",
            "==" | "!=" | "<" | "<=" | ">" | ">=" => "EqualityOperator",
            "and" | "or" => "LogicalOperator",
            "True" | "False" => "BooleanLiteral",
            "not" | "~" => "UnaryOperator",
            "is" | "is not" | "in" | "not in" => "ConditionalExpression",
            "break" | "continue" | "return" => "StatementSwap",
            // Nothing above matched a token, so a `True` or `False` here is a
            // whole condition rewritten to a constant. This sits *below* the
            // token arms on purpose: a relational operator whose replacement is
            // a constant is still relational replacement.
            _ if self.replacement == "True" || self.replacement == "False" => {
                "ConditionalExpression"
            }
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
    ($($token:ident => [$($replacement:literal),+ $(,)?]),+ $(,)?) => {
        fn swapped(kind: TokenKind) -> &'static [&'static str] {
            match kind {
                $(TokenKind::$token => &[$($replacement),+],)+
                _ => &[],
            }
        }

        #[cfg(test)]
        const MUTABLE_TOKENS: &[(TokenKind, &[&str])] =
            &[$((TokenKind::$token, &[$($replacement),+] as &[&str])),+];
    };
}

operators! {
    // arithmetic
    Plus => ["-"],
    Minus => ["+"],
    Star => ["/"],
    Slash => ["*"],
    DoubleSlash => ["/"],
    Percent => ["/"],
    DoubleStar => ["*"],
    // bitwise and shifts
    Amper => ["|"],
    Vbar => ["&"],
    CircumFlex => ["&"],
    LeftShift => [">>"],
    RightShift => ["<<"],
    // Comparison. An ordering operator takes two: the boundary shift, which
    // only a test sitting exactly on the boundary can kill, and the negation,
    // which any test that observes the predicate kills. One replacement was
    // cheaper and left the boundary case untested, and relational replacement
    // is the operator with the best evidence behind it, so it is the wrong
    // place to economise. Equality has no boundary neighbour, so it keeps one.
    EqEqual => ["!="],
    NotEqual => ["=="],
    Less => ["<=", ">="],
    LessEqual => ["<", ">"],
    Greater => [">=", "<="],
    GreaterEqual => [">", "<"],
    // boolean and keyword swaps
    And => ["or"],
    Or => ["and"],
    True => ["False"],
    False => ["True"],
    Break => ["return"],
    Continue => ["break"],
    Is => ["is not"],
    // augmented assignment
    PlusEqual => ["-="],
    MinusEqual => ["+="],
    StarEqual => ["/="],
    SlashEqual => ["*="],
    DoubleSlashEqual => ["/="],
    PercentEqual => ["/="],
    DoubleStarEqual => ["*="],
    AmperEqual => ["|="],
    VbarEqual => ["&="],
    CircumflexEqual => ["&="],
    LeftShiftEqual => [">>="],
    RightShiftEqual => ["<<="],
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

/// Every replacement a token admits. Most give one; an ordering operator and a
/// string give more.
fn replacements(kind: TokenKind, text: &str, after_dot: bool) -> Vec<String> {
    let swaps = swapped(kind);
    if !swaps.is_empty() {
        return swaps.iter().map(|swap| swap.to_string()).collect();
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

/// Which mutants a run is willing to plant, and where.
///
/// Two levers, and the measured one is the second. Choosing operators well has
/// a ceiling of about 13% over sampling mutants at random; choosing where not
/// to apply them took Google's median from 820 mutants per change to 7, and
/// raised the share developers judged worth acting on from 15% to 89%.
pub struct Operators {
    families: Vec<String>,
    arid_names: Vec<String>,
    /// How many mutants the arid names turned away. A silent suppression raises
    /// the score exactly the way a silent exclusion does, so a run has to be
    /// able to say what it decided not to look at.
    suppressed: usize,
    /// How many a disabled family turned away.
    filtered: usize,
}

impl Operators {
    /// Refuses a family name it does not know. A typo would otherwise disable a
    /// whole family, drop its mutants from the pool, and report a higher score
    /// for the loss.
    pub fn new(families: &[String], arid: &[String]) -> Result<Operators> {
        if families.is_empty() {
            bail!("operators is empty, so there is nothing to mutate");
        }
        for family in families {
            if !FAMILIES.contains(&family.as_str()) {
                bail!(
                    "unknown operator family {family:?}, pick from {}",
                    FAMILIES.join(", ")
                );
            }
        }
        Ok(Operators {
            families: families.to_vec(),
            arid_names: arid.to_vec(),
            suppressed: 0,
            filtered: 0,
        })
    }

    /// Every family and no suppression, for a test that is about one operator
    /// rather than about which operators a run enables.
    #[cfg(test)]
    pub(crate) fn everything() -> Operators {
        Operators {
            families: FAMILIES.iter().map(|name| name.to_string()).collect(),
            arid_names: Vec::new(),
            suppressed: 0,
            filtered: 0,
        }
    }

    fn enabled(&self, family: &str) -> bool {
        self.families.iter().any(|listed| listed == family)
    }

    /// Rendered as a trailing clause, empty when nothing was turned away, so a
    /// run of the whole set says what it has always said.
    pub fn note(&self) -> String {
        match (self.filtered, self.suppressed) {
            (0, 0) => String::new(),
            (filtered, 0) => format!(", {filtered} skipped by operators"),
            (0, suppressed) => format!(", {suppressed} skipped as arid"),
            (filtered, suppressed) => {
                format!(", {filtered} skipped by operators and {suppressed} as arid")
            }
        }
    }

    pub fn enumerate_file(&mut self, file: &Path) -> Result<Vec<Mutant>> {
        let source =
            fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
        self.enumerate_source(&source, file)
    }

    pub(crate) fn enumerate_source(&mut self, source: &str, file: &Path) -> Result<Vec<Mutant>> {
        let parsed = ruff_python_parser::parse_module(source)
            .with_context(|| format!("parsing {}", file.display()))?;

        let mut nodes = Nodes::new(&self.arid_names);
        nodes.walk(&parsed.syntax().body);

        let mut found = std::mem::take(&mut nodes.found);
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
                found.push((start, end, replacement));
            }
            previous = Some(kind);
        }

        // Byte order, so `Lines` scans the file once rather than once per
        // mutant. Tokens already arrive in order, but whole-node rewrites do
        // not interleave with them on their own, and the end breaks the tie so
        // two mutants starting on one byte keep their order between runs.
        found.sort_by_key(|(start, end, _)| (*start, *end));

        let mut lines = Lines::default();
        let mut mutants = Vec::new();
        for (start, end, replacement) in found {
            let Some(original) = source.get(start..end) else {
                continue;
            };
            if replacement == original {
                continue;
            }
            if nodes.suppresses(start, end) {
                self.suppressed += 1;
                continue;
            }
            let mutant = Mutant {
                id: 0,
                file: file.to_path_buf(),
                line: lines.at(source, start),
                byte_start: start,
                byte_end: end,
                original: original.to_string(),
                replacement,
            };
            if !self.enabled(mutant.mutator()) {
                self.filtered += 1;
                continue;
            }
            mutants.push(mutant);
        }
        Ok(mutants)
    }
}

/// The half of enumeration a token stream cannot see: whole statements, whole
/// conditions, and the call sites not worth mutating at all.
struct Nodes<'a> {
    arid_names: &'a [String],
    /// Byte ranges holding nothing worth a mutant.
    arid: Vec<(usize, usize)>,
    /// Whole-node rewrites, as `(start, end, replacement)`.
    found: Vec<(usize, usize, String)>,
}

/// Which constants a condition may become. A `while` gets only `False`:
/// `while True` on a loop that used to end never ends, and a hang costs the
/// whole timeout budget to teach what `False` teaches in milliseconds.
#[derive(Clone, Copy)]
enum Constants {
    Both,
    FalseOnly,
}

impl<'a> Nodes<'a> {
    fn new(arid_names: &'a [String]) -> Nodes<'a> {
        Nodes {
            arid_names,
            arid: Vec::new(),
            found: Vec::new(),
        }
    }

    /// Whether a mutant of this range sits inside something arid.
    fn suppresses(&self, start: usize, end: usize) -> bool {
        self.arid
            .iter()
            .any(|(from, to)| start >= *from && end <= *to)
    }

    fn walk(&mut self, body: &[Stmt]) {
        for statement in body {
            self.statement(statement);
        }
    }

    fn statement(&mut self, statement: &Stmt) {
        if self.is_arid(statement) {
            self.arid.push(span(statement.range()));
            return;
        }
        if deletable(statement) {
            let (start, end) = span(statement.range());
            self.found.push((start, end, "pass".to_string()));
        }
        match statement {
            Stmt::If(node) => {
                self.condition(&node.test, Constants::Both);
                self.walk(&node.body);
                for clause in &node.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.condition(test, Constants::Both);
                    }
                    self.walk(&clause.body);
                }
            }
            Stmt::While(node) => {
                self.condition(&node.test, Constants::FalseOnly);
                self.walk(&node.body);
                self.walk(&node.orelse);
            }
            Stmt::For(node) => {
                self.walk(&node.body);
                self.walk(&node.orelse);
            }
            Stmt::With(node) => self.walk(&node.body),
            Stmt::Try(node) => {
                self.walk(&node.body);
                for handler in &node.handlers {
                    let ExceptHandler::ExceptHandler(handler) = handler;
                    self.walk(&handler.body);
                }
                self.walk(&node.orelse);
                self.walk(&node.finalbody);
            }
            Stmt::Match(node) => {
                for case in &node.cases {
                    self.walk(&case.body);
                }
            }
            // `__repr__` and `__str__` exist to be read by a person. Mutating
            // one changes a debugging string and nothing a caller depends on,
            // which is what arid means.
            Stmt::FunctionDef(node) => match node.name.as_str() {
                "__repr__" | "__str__" => self.arid.push(span(node.range)),
                _ => self.walk(&node.body),
            },
            Stmt::ClassDef(node) => self.walk(&node.body),
            _ => {}
        }
    }

    /// Replace a whole condition with a constant. This is the operator family
    /// every yardstick in the literature supports and no token swap can
    /// express: the token stream can turn `and` into `or`, but only the tree
    /// knows where a condition starts and stops.
    fn condition(&mut self, test: &Expr, allow: Constants) {
        // A condition that is already a constant learns nothing from becoming
        // one, and a deliberate `while True:` is not a fault waiting to be
        // found.
        if test.is_literal_expr() {
            return;
        }
        let (start, end) = span(test.range());
        self.found.push((start, end, "False".to_string()));
        if matches!(allow, Constants::Both) {
            self.found.push((start, end, "True".to_string()));
        }
    }

    fn is_arid(&self, statement: &Stmt) -> bool {
        if self.arid_names.is_empty() {
            return false;
        }
        let Stmt::Expr(node) = statement else {
            return false;
        };
        let Expr::Call(call) = node.value.as_ref() else {
            return false;
        };
        callee_names(&call.func)
            .iter()
            .any(|segment| self.arid_names.iter().any(|arid| arid == segment))
    }
}

/// Every segment of a dotted callee, so `self.logger.info(...)` answers for
/// `logger` as readily as `log.info(...)` answers for `log`.
fn callee_names(func: &Expr) -> Vec<&str> {
    let mut names = Vec::new();
    let mut current = func;
    loop {
        match current {
            Expr::Attribute(attribute) => {
                names.push(attribute.attr.as_str());
                current = attribute.value.as_ref();
            }
            Expr::Name(name) => {
                names.push(name.id.as_str());
                return names;
            }
            _ => return names,
        }
    }
}

/// A statement worth removing, replaced by `pass` so a block that loses its
/// only statement is still a block.
///
/// Deletion has the best cost-effectiveness evidence in the literature and is
/// the one family the canonical operator sets leave out. What it must not touch
/// is anything whose removal is either free to detect or impossible to detect.
/// An import or a definition takes every use of the name with it, so any test
/// that touches the module kills the mutant on sight and learns nothing. A bare
/// `return` is the mirror image: the function already returns `None` without
/// it, so removing it is an equivalent mutant by construction. A docstring, an
/// `...` stub body and a bare literal are that same case.
fn deletable(statement: &Stmt) -> bool {
    match statement {
        Stmt::Return(node) => node.value.is_some(),
        Stmt::AnnAssign(node) => node.value.is_some(),
        Stmt::Expr(node) => !node.value.is_literal_expr(),
        Stmt::Assign(_)
        | Stmt::AugAssign(_)
        | Stmt::Raise(_)
        | Stmt::Assert(_)
        | Stmt::Delete(_)
        | Stmt::If(_)
        | Stmt::For(_)
        | Stmt::While(_)
        | Stmt::With(_)
        | Stmt::Try(_)
        | Stmt::Match(_)
        | Stmt::Break(_)
        | Stmt::Continue(_) => true,
        _ => false,
    }
}

fn span(range: TextRange) -> (usize, usize) {
    (range.start().to_usize(), range.end().to_usize())
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
        Operators::everything()
            .enumerate_source(source, Path::new("x.py"))
            .unwrap()
    }

    /// The free form, for the tests that care about the path rather than about
    /// which operators are enabled.
    fn enumerate_source(source: &str, file: &Path) -> Result<Vec<Mutant>> {
        Operators::everything().enumerate_source(source, file)
    }

    /// What a run with no config of its own would plant.
    fn by_default(source: &str) -> Vec<Mutant> {
        let families: Vec<String> = DEFAULT_FAMILIES.iter().map(|f| f.to_string()).collect();
        let arid: Vec<String> = DEFAULT_ARID.iter().map(|n| n.to_string()).collect();
        Operators::new(&families, &arid)
            .unwrap()
            .enumerate_source(source, Path::new("x.py"))
            .unwrap()
    }

    fn changes(source: &str) -> Vec<(String, String)> {
        mutants(source)
            .into_iter()
            .map(|m| (m.original, m.replacement))
            .collect()
    }

    #[test]
    fn every_listed_token_produces_its_replacements() {
        for (kind, replacements) in MUTABLE_TOKENS {
            assert_eq!(swapped(*kind), *replacements, "{kind:?}");
        }
        assert!(swapped(TokenKind::Name).is_empty());
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
        // The `+` is text, not an operator, so nothing may be planted on it.
        assert!(!found.iter().any(|(original, _)| original == "+"));
        assert!(found.iter().any(|(original, _)| original == "'a + b'"));
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
        for (_, replacements) in MUTABLE_TOKENS {
            for replacement in *replacements {
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

    /// Three of the seven mutants a relational clause admits subsume the other
    /// four, and one replacement per operator was under that mark rather than
    /// over it: only the negation was planted, so nothing ever landed on the
    /// boundary that off-by-one faults live on.
    #[test]
    fn an_ordering_operator_gets_the_boundary_and_the_negation() {
        let found = changes("if a < b:\n    pass\n");
        assert!(found.contains(&("<".into(), "<=".into())), "{found:?}");
        assert!(found.contains(&("<".into(), ">=".into())), "{found:?}");
        // Equality has no boundary neighbour, so it keeps its one swap.
        let equality = changes("if a == b:\n    pass\n");
        let swaps: Vec<&(String, String)> = equality.iter().filter(|(o, _)| o == "==").collect();
        assert_eq!(swaps.len(), 1, "{equality:?}");
    }

    #[test]
    fn deletes_a_statement_by_replacing_it_with_pass() {
        let found = changes("def f(x):\n    x.append(1)\n    return x\n");
        assert!(
            found.contains(&("x.append(1)".into(), "pass".into())),
            "{found:?}"
        );
        assert!(
            found.contains(&("return x".into(), "pass".into())),
            "{found:?}"
        );
    }

    /// A whole block goes as readily as one statement: that is what makes
    /// deletion cheap enough to be worth its evidence.
    #[test]
    fn deletes_a_whole_block_and_the_statements_inside_it() {
        let source = "def f(x):\n    if x:\n        return 1\n    return 2\n";
        let found = changes(source);
        assert!(
            found.contains(&("if x:\n        return 1".into(), "pass".into())),
            "{found:?}"
        );
        assert!(
            found.contains(&("return 1".into(), "pass".into())),
            "{found:?}"
        );
    }

    /// Deleting a name every other line depends on is killed on sight by any
    /// test that imports the module, and a bare `return` changes nothing at
    /// all. Neither is worth a test run.
    #[test]
    fn leaves_alone_the_statements_deletion_cannot_learn_from() {
        let source = "\
import os


def f():
    \"a docstring\"
    return
";
        let deletions: Vec<(String, String)> = changes(source)
            .into_iter()
            .filter(|(_, replacement)| replacement == "pass")
            .collect();
        assert!(deletions.is_empty(), "{deletions:?}");
    }

    #[test]
    fn replaces_a_condition_with_a_constant() {
        let found = changes("if a and b:\n    pass\n");
        assert!(
            found.contains(&("a and b".into(), "True".into())),
            "{found:?}"
        );
        assert!(
            found.contains(&("a and b".into(), "False".into())),
            "{found:?}"
        );
        // An `elif` is a condition too, and the token stream cannot see it.
        let chained = changes("if a:\n    pass\nelif b:\n    pass\n");
        assert!(
            chained.contains(&("b".into(), "False".into())),
            "{chained:?}"
        );
    }

    /// A timeout counts as detected, so a mutant that hangs is not merely slow:
    /// it spends the entire budget to report what `False` reports at once.
    #[test]
    fn a_while_condition_never_becomes_true() {
        let found = changes("while ready:\n    pass\n");
        assert!(
            found.contains(&("ready".into(), "False".into())),
            "{found:?}"
        );
        assert!(
            !found.contains(&("ready".into(), "True".into())),
            "{found:?}"
        );
        // A condition that is already constant has nothing to learn. Its
        // literal is still swapped; what must not appear is a second mutant
        // rewriting the condition into the constant it already is.
        assert!(
            !mutants("while True:\n    break\n")
                .iter()
                .any(|m| m.mutator() == "ConditionalExpression")
        );
    }

    #[test]
    fn arid_calls_and_dunder_repr_are_left_alone() {
        let source = "\
class Thing:
    def __repr__(self):
        return \"Thing(%d)\" % (1 + 1)

    def run(self, n):
        self.logger.info(\"n is %d\", n + 1)
        print(n * 2)
        return n - 1
";
        let lines: Vec<u32> = by_default(source).iter().map(|m| m.line).collect();
        // Only `run`'s own return is left, and it is mutated and deleted.
        assert_eq!(lines, vec![8, 8], "{lines:?}");
    }

    /// An arid list is only worth having if a run says what it cost.
    #[test]
    fn the_note_reports_what_was_turned_away() {
        let families: Vec<String> = DEFAULT_FAMILIES.iter().map(|f| f.to_string()).collect();
        let arid: Vec<String> = DEFAULT_ARID.iter().map(|n| n.to_string()).collect();
        let mut operators = Operators::new(&families, &arid).unwrap();
        operators
            .enumerate_source("print(1 + 1)\ns = 'x'\n", Path::new("x.py"))
            .unwrap();
        let note = operators.note();
        assert!(note.contains("skipped by operators"), "{note}");
        assert!(note.contains("3 as arid"), "{note}");
        assert!(Operators::everything().note().is_empty());
    }

    /// The three families the evidence does not support are off unless a
    /// project turns them back on.
    #[test]
    fn the_default_set_leaves_out_what_the_evidence_does_not_support() {
        let found = by_default("def f(n):\n    return not n + 1\n");
        let families: Vec<&str> = found.iter().map(|m| m.mutator()).collect();
        assert!(!families.contains(&"UnaryOperator"), "{families:?}");
        assert!(!families.contains(&"NumberLiteral"), "{families:?}");
        assert!(families.contains(&"ArithmeticOperator"), "{families:?}");
        assert!(families.contains(&"BlockStatement"), "{families:?}");
        // And everything is still reachable by asking for it.
        assert!(
            mutants("def f(n):\n    return not n + 1\n")
                .iter()
                .any(|m| m.mutator() == "UnaryOperator")
        );
    }

    /// A family name angelo does not know has to stop the run. Accepted
    /// silently it would disable a whole family, shrink the pool, and report a
    /// higher score for the loss.
    #[test]
    fn an_unknown_family_is_refused() {
        let wrong = vec!["ArithmeticOperator".to_string(), "Relational".to_string()];
        let error = Operators::new(&wrong, &[])
            .err()
            .expect("an unknown family stops the run")
            .to_string();
        assert!(error.contains("Relational"), "{error}");
        assert!(Operators::new(&[], &[]).is_err());
    }

    /// `operators` in angelo.conf is checked against this list, so a family
    /// `mutator` can name and the list cannot is a config key nobody can set.
    #[test]
    fn every_family_is_listed() {
        let source = "\
class Thing:
    def run(self, n, s, flag):
        n += 1
        n = n + 1 & 2 << 3
        if flag and n > 1 is None:
            return not True
        while flag:
            break
        return s.lower() + 'x' + str(1.5)
";
        for mutant in mutants(source) {
            assert!(
                FAMILIES.contains(&mutant.mutator()),
                "{:?} -> {:?} is a {} that FAMILIES does not list",
                mutant.original,
                mutant.replacement,
                mutant.mutator()
            );
        }
        for family in DEFAULT_FAMILIES {
            assert!(FAMILIES.contains(family), "{family}");
        }
    }

    /// A deleted `break` is a deletion. Its original is a token the swap table
    /// also knows, so the family cannot be read off the original alone.
    #[test]
    fn a_deleted_statement_is_named_for_the_deletion() {
        let found = mutants("while x:\n    break\n");
        let deleted = found
            .iter()
            .find(|m| m.original == "break" && m.replacement == "pass")
            .expect("the deletion");
        assert_eq!(deleted.mutator(), "BlockStatement");
        let swapped = found
            .iter()
            .find(|m| m.original == "break" && m.replacement == "return")
            .expect("the swap");
        assert_eq!(swapped.mutator(), "StatementSwap");
    }

    /// A relational operator replaced by a constant is still relational
    /// replacement, so the condition rule must not claim it.
    #[test]
    fn a_condition_is_named_apart_from_the_operators_inside_it() {
        let found = mutants("if a > b:\n    pass\n");
        let condition = found
            .iter()
            .find(|m| m.original == "a > b")
            .expect("the condition");
        assert_eq!(condition.mutator(), "ConditionalExpression");
        let operator = found.iter().find(|m| m.original == ">").expect("the >");
        assert_eq!(operator.mutator(), "EqualityOperator");
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
