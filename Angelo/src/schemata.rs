//! Every mutant of a file compiled into that file at once.
//!
//! Splicing writes one mutant into the source, so the process has to re-import
//! the project to see it. Schemata write them all in at once, each in its own
//! copy of the function it belongs to, and pick one at run time from an
//! environment variable. Nothing is re-imported between mutants, which is the
//! whole saving; on a project with a large import graph it is most of the run.
//!
//! ```python
//! def add__angelo_orig(a, b):
//!     return a + b
//!
//! def add__angelo_7(a, b):
//!     return a - b
//!
//! add__angelo_mutants = {7: add__angelo_7}
//!
//! def add(*a, _angelo_orig=add__angelo_orig, _angelo_mutants=add__angelo_mutants, **kw):
//!     return _angelo_pick(_angelo_orig, _angelo_mutants)(*a, **kw)
//! ```
//!
//! The original and the mutants are copied whole, so a mutant anywhere inside a
//! function works without understanding what it changed: defaults, nested
//! functions and comprehensions all come along as text.
//!
//! Not every mutant fits. Module-level code, class attributes and decorators
//! have no function to copy, and those keep using the splice path, which is
//! always correct and merely slower.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Expr, Stmt, StmtFunctionDef};
use ruff_text_size::Ranged;

use crate::mutate::Mutant;

/// The runtime the generated code calls, written beside it in the worker copy.
const RUNTIME: &str = include_str!("runner/angelo_rt.py");
pub const RUNTIME_FILE: &str = "_angelo_rt.py";
/// Read by the runtime; holds the ids of the mutants that should be live.
pub const ACTIVE_VAR: &str = "ANGELO_MUTANTS";
/// Set to a directory to keep a readable copy of everything generated.
const DUMP_VAR: &str = "ANGELO_DUMP_SCHEMATA";

/// The function one hosted mutant lives in. Only one member of a family can be
/// live at a time — the wrapper calls one copy — so two mutants sharing a host
/// cannot be judged by the same run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Host(usize);

/// The rewritten source of every file that could host at least one mutant.
pub struct Schemata {
    generated: HashMap<PathBuf, String>,
    hosted: HashMap<i64, Host>,
}

impl Schemata {
    /// Rewrite every file that holds a mutant. A file that cannot host any is
    /// left out entirely, so the copy keeps the original and the splice path
    /// handles it.
    pub fn build(project_root: &Path, mutants: &[Mutant]) -> Result<Schemata> {
        let mut by_file: HashMap<&Path, Vec<&Mutant>> = HashMap::new();
        for mutant in mutants {
            by_file
                .entry(mutant.file.as_path())
                .or_default()
                .push(mutant);
        }

        let mut schemata = Schemata {
            generated: HashMap::new(),
            hosted: HashMap::new(),
        };
        let mut next_host = 0;
        for (file, mutants) in by_file {
            let path = project_root.join(file);
            let source = fs::read_to_string(&path)
                .with_context(|| format!("reading {} to build schemata", path.display()))?;
            // A file angelo cannot parse has no functions to copy; the splice
            // path will report it the same way it always has.
            let Ok(rewritten) = rewrite(&source, &mutants) else {
                continue;
            };
            let Some(rewritten) = rewritten else {
                continue;
            };
            for family in rewritten.hosted {
                let host = Host(next_host);
                next_host += 1;
                schemata
                    .hosted
                    .extend(family.into_iter().map(|id| (id, host)));
            }
            schemata
                .generated
                .insert(file.to_path_buf(), rewritten.source);
        }
        Ok(schemata)
    }

    pub fn hosts(&self, mutant: &Mutant) -> bool {
        self.hosted.contains_key(&mutant.id)
    }

    /// Which function would have to switch to judge this mutant. Two mutants
    /// that answer the same thing need two runs.
    pub fn host(&self, mutant: &Mutant) -> Option<Host> {
        self.hosted.get(&mutant.id).copied()
    }

    pub fn hosted_count(&self) -> usize {
        self.hosted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hosted.is_empty()
    }

    /// The rewritten source for a file, for whoever has to put it back after
    /// borrowing the copy for a splice.
    pub fn generated_for(&self, file: &Path) -> Option<&str> {
        self.generated.get(file).map(String::as_str)
    }

    /// Write the generated files over a worker's copy, plus the runtime they
    /// import. The copy root is on PYTHONPATH already, so a flat module name
    /// resolves from anywhere in the tree.
    pub fn write_into(&self, copy_root: &Path) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        let runtime = copy_root.join(RUNTIME_FILE);
        fs::write(&runtime, RUNTIME).with_context(|| format!("writing {}", runtime.display()))?;
        for (file, source) in &self.generated {
            let path = copy_root.join(file);
            fs::write(&path, source)
                .with_context(|| format!("writing schemata into {}", path.display()))?;
        }
        self.dump()
    }

    /// Generated code is the one thing here nobody can read in the repository,
    /// and a mistake in it turns every mutant in the file into an `error`, which
    /// reads exactly like a broken test command. `ANGELO_DUMP_SCHEMATA=<dir>`
    /// writes it out where it can be looked at and run.
    fn dump(&self) -> Result<()> {
        let Some(target) = std::env::var_os(DUMP_VAR) else {
            return Ok(());
        };
        let target = PathBuf::from(target);
        for (file, source) in &self.generated {
            let path = target.join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::write(&path, source).with_context(|| format!("dumping {}", path.display()))?;
        }
        fs::write(target.join(RUNTIME_FILE), RUNTIME)
            .with_context(|| format!("dumping the runtime into {}", target.display()))?;
        Ok(())
    }

    /// The value of the environment variable that makes exactly these mutants
    /// live. An empty set runs the original code, which is how the generated
    /// file behaves for any test that is not judging a mutant.
    pub fn active_value(mutants: &[&Mutant]) -> String {
        mutants
            .iter()
            .map(|mutant| mutant.id.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

struct Rewritten {
    source: String,
    /// One entry per function, holding the mutants that function took. The
    /// grouping is the point: it is what stops two of them sharing a run.
    hosted: Vec<Vec<i64>>,
}

/// One edit to make to the file, applied back to front so earlier offsets stay
/// valid, the same discipline `runner::splice_all` uses.
struct Edit {
    start: usize,
    end: usize,
    text: String,
}

fn rewrite(source: &str, mutants: &[&Mutant]) -> Result<Option<Rewritten>> {
    let parsed = ruff_python_parser::parse_module(source).context("parsing for schemata")?;
    let mut functions = Vec::new();
    collect_functions(parsed.syntax().body.iter(), source, &mut functions);

    let mut edits = Vec::new();
    let mut hosted = Vec::new();
    for function in &functions {
        let mine: Vec<&Mutant> = mutants
            .iter()
            .copied()
            .filter(|mutant| function.holds(mutant))
            .collect();
        if mine.is_empty() {
            continue;
        }
        let Some((text, taken)) = function.family(source, &mine) else {
            continue;
        };
        hosted.push(taken);
        edits.push(Edit {
            start: function.replace_from,
            end: function.end,
            text,
        });
    }
    if hosted.is_empty() {
        return Ok(None);
    }

    edits.push(runtime_import(source, &parsed.syntax().body));
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
    let mut rewritten = source.to_string();
    for edit in edits {
        rewritten.replace_range(edit.start..edit.end, &edit.text);
    }
    Ok(Some(Rewritten {
        source: rewritten,
        hosted,
    }))
}

/// `from __future__ import ...` has to stay the first statement, and a module
/// docstring has to stay a docstring, so the import goes after both.
fn runtime_import(source: &str, body: &[Stmt]) -> Edit {
    let mut at = 0;
    for statement in body {
        let keeps_its_place = match statement {
            Stmt::Expr(expression) => matches!(*expression.value, Expr::StringLiteral(_)),
            Stmt::ImportFrom(import) => import
                .module
                .as_ref()
                .is_some_and(|m| m.as_str() == "__future__"),
            _ => false,
        };
        if !keeps_its_place {
            break;
        }
        at = line_after(source, statement.range().end().to_usize());
    }
    Edit {
        start: at,
        end: at,
        text: format!(
            "from {} import _angelo_pick\n",
            RUNTIME_FILE.trim_end_matches(".py")
        ),
    }
}

fn line_after(source: &str, offset: usize) -> usize {
    match source[offset..].find('\n') {
        Some(at) => offset + at + 1,
        None => source.len(),
    }
}

/// A function angelo can copy: defined at module level or straight inside a
/// class, so its copies can sit beside it in the same namespace.
struct Function {
    name: String,
    name_start: usize,
    name_end: usize,
    /// Where the replacement starts: the first decorator, or the `def`.
    replace_from: usize,
    def_start: usize,
    /// The first statement, so the signature is not treated as hostable.
    body_start: usize,
    end: usize,
    indent: String,
    shape: Shape,
}

/// Which wrapper the function needs. A plain wrapper returns whatever the call
/// returns, which is already right for a coroutine, but then
/// `inspect.iscoroutinefunction` says no and frameworks that branch on it take
/// the wrong path, so async and generator functions get their own shape.
enum Shape {
    Plain,
    Async,
    Generator,
}

impl Function {
    /// From the first statement, not from the `def`. A mutant in a default
    /// argument runs when the copy is defined rather than when it is called, so
    /// one that raises would take the whole module down with it and every other
    /// mutant in the file. Those stay spliced.
    fn holds(&self, mutant: &Mutant) -> bool {
        mutant.byte_start >= self.body_start && mutant.byte_end <= self.end
    }

    /// The original, one copy per mutant it can take, the lookup table and the
    /// wrapper. Also the ids it took: a mutant whose copy will not compile is
    /// left behind for the splice path rather than dropped.
    fn family(&self, source: &str, mutants: &[&Mutant]) -> Option<(String, Vec<i64>)> {
        let mut out = self.copy(source, &self.orig_name(), None)?;
        let mut hosted = Vec::new();
        for mutant in mutants {
            let Some(copy) = self.copy(source, &self.mutant_name(mutant.id), Some(mutant)) else {
                continue;
            };
            out.push_str(&copy);
            hosted.push(mutant.id);
        }
        if hosted.is_empty() {
            return None;
        }

        let entries: Vec<String> = hosted
            .iter()
            .map(|id| format!("{id}: {}", self.mutant_name(*id)))
            .collect();
        out.push_str(&format!(
            "{}{} = {{{}}}\n\n",
            self.indent,
            self.mutants_name(),
            entries.join(", ")
        ));

        // Decorators belong on the wrapper: it is the one the outside world
        // calls, so @property and @classmethod have to wrap it and not a copy.
        // The slice starts at the line, so it already carries the indentation
        // the replaced text had.
        out.push_str(&source[self.replace_from..self.def_start]);
        out.push_str(&self.wrapper());
        Some((out, hosted))
    }

    /// One member of the family: the function's own text, renamed, with at most
    /// one mutant spliced into it.
    fn copy(&self, source: &str, name: &str, mutant: Option<&Mutant>) -> Option<String> {
        let mut text = source.get(self.def_start..self.end)?.to_string();
        let mut edits = vec![Edit {
            start: self.name_start - self.def_start,
            end: self.name_end - self.def_start,
            text: name.to_string(),
        }];
        if let Some(mutant) = mutant {
            let start = mutant.byte_start - self.def_start;
            let end = mutant.byte_end - self.def_start;
            if text.get(start..end) != Some(mutant.original.as_str()) {
                return None;
            }
            edits.push(Edit {
                start,
                end,
                text: mutant.replacement.clone(),
            });
        }
        edits.sort_by_key(|edit| std::cmp::Reverse(edit.start));
        for edit in edits {
            text.replace_range(edit.start..edit.end, &edit.text);
        }
        let text = text.trim_end_matches(['\n', ' ', '\t']);
        // A mutant that breaks the syntax is harmless when it is spliced: it
        // errors on its own run and sits outside the score. Compiled into the
        // file it would break every other mutant there too, so it is checked
        // now and left to the splice path if it will not parse. `*args` mutated
        // to `/args` is the one that found this.
        if mutant.is_some()
            && ruff_python_parser::parse_module(&dedented(text, &self.indent)).is_err()
        {
            return None;
        }
        Some(format!("{}{}\n\n", self.indent, text))
    }

    /// Both the original and the table are captured as default arguments rather
    /// than looked up by name: a method's body cannot see its own class body,
    /// so a bare name would resolve at call time and fail.
    fn wrapper(&self) -> String {
        let body = self.indent.clone() + "    ";
        let call = "_angelo_pick(_angelo_orig, _angelo_mutants, _angelo_cache)(*_angelo_args, **_angelo_kwargs)";
        let (keyword, statement) = match self.shape {
            Shape::Plain => ("def", format!("return {call}")),
            Shape::Async => ("async def", format!("return await {call}")),
            // PEP 380: the parenthesised form forwards the generator's return
            // value, which a bare `yield from` would swallow.
            Shape::Generator => ("def", format!("return (yield from {call})")),
        };
        // `_angelo_cache` is a mutable default on purpose: one list per
        // function, created once, which is the cheapest per-function storage
        // Python has. The runtime uses it to answer in constant time.
        format!(
            "{keyword} {}(*_angelo_args, _angelo_orig={}, _angelo_mutants={}, _angelo_cache=[-1, None], **_angelo_kwargs):\n{body}{statement}\n",
            self.name,
            self.orig_name(),
            self.mutants_name(),
        )
    }

    fn orig_name(&self) -> String {
        format!("{}__angelo_orig", self.name)
    }

    fn mutant_name(&self, id: i64) -> String {
        format!("{}__angelo_{id}", self.name)
    }

    fn mutants_name(&self) -> String {
        format!("{}__angelo_mutants", self.name)
    }
}

/// Module level and class bodies only. A nested function is never collected,
/// but it still travels inside whichever outer function contains it, so its
/// mutants are hosted all the same.
fn collect_functions<'a>(
    body: impl Iterator<Item = &'a Stmt>,
    source: &str,
    found: &mut Vec<Function>,
) {
    for statement in body {
        match statement {
            Stmt::FunctionDef(function) => {
                if let Some(collected) = describe(function, source) {
                    found.push(collected);
                }
            }
            Stmt::ClassDef(class) => collect_functions(class.body.iter(), source, found),
            _ => {}
        }
    }
}

fn describe(function: &StmtFunctionDef, source: &str) -> Option<Function> {
    let name = function.name.as_str().to_string();
    let name_start = function.name.range().start().to_usize();
    let name_end = function.name.range().end().to_usize();

    // The name always follows the `def` keyword, so searching back from it
    // finds the keyword whether or not the node's range covers the decorators.
    let mut def_start = source.get(..name_start)?.rfind("def ")?;
    if function.is_async {
        let head = source.get(..def_start)?.trim_end();
        if !head.ends_with("async") {
            return None;
        }
        def_start = head.len() - "async".len();
    }

    let indent = indent_of(source, def_start)?;
    let decorators = function
        .decorator_list
        .iter()
        .map(|d| d.range().start().to_usize())
        .min()
        .filter(|first| *first < def_start);
    // A decorator naming the function it decorates is rebuilding it, the
    // `@x.setter` pattern. The wrapper would be assigned before the name it
    // extends exists, so leave the whole family alone.
    if let Some(first) = decorators
        && source.get(first..def_start)?.contains(&name)
    {
        return None;
    }
    // Replace from the start of the line, so the indentation already in the
    // file is consumed rather than added to the indentation the family emits.
    let replace_from = line_start(source, decorators.unwrap_or(def_start));

    let mut yields = FindsYield::default();
    for statement in &function.body {
        yields.visit_stmt(statement);
    }
    let shape = match (function.is_async, yields.found) {
        // An async generator needs both `async def` and `yield from`, which do
        // not combine. Rare enough to leave on the splice path.
        (true, true) => return None,
        (true, false) => Shape::Async,
        (false, true) => Shape::Generator,
        (false, false) => Shape::Plain,
    };

    Some(Function {
        name,
        name_start,
        name_end,
        replace_from,
        def_start,
        body_start: function.body.first()?.range().start().to_usize(),
        end: function.range().end().to_usize(),
        indent,
        shape,
    })
}

/// The whitespace in front of the definition. None when something other than
/// whitespace shares the line, which a one-line compound statement can do and
/// which the generated block could not reproduce.
fn indent_of(source: &str, offset: usize) -> Option<String> {
    let indent = source.get(line_start(source, offset)..offset)?;
    indent
        .chars()
        .all(char::is_whitespace)
        .then(|| indent.to_string())
}

fn line_start(source: &str, offset: usize) -> usize {
    source[..offset].rfind('\n').map(|at| at + 1).unwrap_or(0)
}

/// A method's text is not a module until its indentation comes off. The first
/// line already starts at the `def`, so only the rest has a prefix to lose.
fn dedented(text: &str, indent: &str) -> String {
    if indent.is_empty() {
        return text.to_string();
    }
    text.lines()
        .map(|line| line.strip_prefix(indent).unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a function is a generator. A nested function's `yield` belongs to
/// that function, so the walk stops at every definition it meets.
#[derive(Default)]
struct FindsYield {
    found: bool,
}

impl<'a> Visitor<'a> for FindsYield {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
            return;
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if matches!(expr, Expr::Yield(_) | Expr::YieldFrom(_)) {
            self.found = true;
            return;
        }
        if matches!(expr, Expr::Lambda(_)) {
            return;
        }
        visitor::walk_expr(self, expr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate;

    /// Enumerate a source the way a real run does, then keep the one mutant
    /// whose original token matches, so the tests read as behaviour rather than
    /// as byte offsets.
    fn mutant_for(source: &str, original: &str, replacement: &str, id: i64) -> Mutant {
        let mut found = mutate::enumerate_source(source, Path::new("m.py")).unwrap();
        let at = found
            .iter()
            .position(|m| m.original == original && m.replacement == replacement)
            .unwrap_or_else(|| panic!("no {original} -> {replacement} mutant in {source}"));
        let mut mutant = found.swap_remove(at);
        mutant.id = id;
        mutant
    }

    fn generated(source: &str, mutants: &[&Mutant]) -> String {
        rewrite(source, mutants)
            .unwrap()
            .expect("nothing was hosted")
            .source
    }

    #[test]
    fn a_function_becomes_an_original_a_mutant_and_a_wrapper() {
        let source = "def add(a, b):\n    return a + b\n";
        let mutant = mutant_for(source, "+", "-", 7);
        let out = generated(source, &[&mutant]);
        assert!(out.contains("def add__angelo_orig(a, b):\n    return a + b"));
        assert!(out.contains("def add__angelo_7(a, b):\n    return a - b"));
        assert!(out.contains("add__angelo_mutants = {7: add__angelo_7}"));
        assert!(out.contains("def add(*_angelo_args, _angelo_orig=add__angelo_orig"));
        assert!(out.starts_with("from _angelo_rt import _angelo_pick\n"));
    }

    #[test]
    fn a_method_keeps_its_indentation_and_its_class() {
        let source = "class C:\n    def m(self, x):\n        return x * 2\n";
        let mutant = mutant_for(source, "*", "/", 3);
        let out = generated(source, &[&mutant]);
        assert!(out.contains("    def m__angelo_orig(self, x):\n        return x * 2"));
        assert!(out.contains("    def m__angelo_3(self, x):\n        return x / 2"));
        assert!(out.contains("    m__angelo_mutants = {3: m__angelo_3}"));
        assert!(out.contains("    def m(*_angelo_args,"));
    }

    /// Several mutants in one function share the family: that is the entire
    /// point, one import serving every mutant the file holds.
    #[test]
    fn several_mutants_of_one_function_share_the_family() {
        let source = "def f(a, b):\n    return a + b * 2\n";
        let plus = mutant_for(source, "+", "-", 1);
        let star = mutant_for(source, "*", "/", 2);
        let out = generated(source, &[&plus, &star]);
        assert!(out.contains("return a - b * 2"));
        assert!(out.contains("return a + b / 2"));
        assert!(out.contains("f__angelo_mutants = {1: f__angelo_1, 2: f__angelo_2}"));
    }

    #[test]
    fn an_async_function_stays_awaitable() {
        let source = "async def go(x):\n    return x + 1\n";
        let mutant = mutant_for(source, "+", "-", 4);
        let out = generated(source, &[&mutant]);
        assert!(out.contains("async def go__angelo_orig(x):"));
        assert!(out.contains("async def go(*_angelo_args,"));
        assert!(out.contains("return await _angelo_pick"));
    }

    #[test]
    fn a_generator_still_yields() {
        let source = "def counter(n):\n    for i in range(n):\n        yield i + 1\n";
        let mutant = mutant_for(source, "+", "-", 5);
        let out = generated(source, &[&mutant]);
        assert!(out.contains("def counter(*_angelo_args,"));
        assert!(out.contains("return (yield from _angelo_pick"));
    }

    #[test]
    fn decorators_move_onto_the_wrapper() {
        let source = "class C:\n    @property\n    def size(self):\n        return 1 + 1\n";
        let mutant = mutant_for(source, "+", "-", 9);
        let out = generated(source, &[&mutant]);
        // The copies must not be decorated, or @property wraps them too.
        assert!(out.contains("    def size__angelo_orig(self):"));
        assert!(!out.contains("@property\n    def size__angelo_orig"));
        assert!(out.contains("    @property\n    def size(*_angelo_args,"));
    }

    /// `@size.setter` names the function it decorates, so the family would
    /// assign the wrapper before the property it extends exists.
    #[test]
    fn a_decorator_naming_its_own_function_is_left_alone() {
        let source = "class C:\n    @size.setter\n    def size(self, v):\n        self.x = v + 1\n";
        let mutant = mutant_for(source, "+", "-", 2);
        assert!(rewrite(source, &[&mutant]).unwrap().is_none());
    }

    /// Module-level code has no function to copy. It has to stay on the splice
    /// path rather than be silently dropped from the run.
    #[test]
    fn module_level_mutants_are_not_hosted() {
        let source = "TIMEOUT = 30 + 1\n";
        let mutant = mutant_for(source, "+", "-", 1);
        assert!(rewrite(source, &[&mutant]).unwrap().is_none());
    }

    #[test]
    fn a_mutant_inside_a_nested_function_travels_with_the_outer_one() {
        let source = "def outer():\n    def inner(x):\n        return x + 1\n    return inner\n";
        let mutant = mutant_for(source, "+", "-", 6);
        let out = generated(source, &[&mutant]);
        assert!(out.contains("def outer__angelo_6():"));
        assert!(out.contains("return x - 1"));
    }

    /// A nested generator does not make its enclosing function one.
    #[test]
    fn a_nested_yield_does_not_make_the_outer_function_a_generator() {
        let source = "def outer(n):\n    def gen():\n        yield n + 1\n    return gen\n";
        let mutant = mutant_for(source, "+", "-", 1);
        let out = generated(source, &[&mutant]);
        assert!(out.contains("return _angelo_pick"));
        assert!(!out.contains("yield from"));
    }

    #[test]
    fn the_runtime_import_goes_after_future_imports_and_the_docstring() {
        let source = "\"\"\"Docs.\"\"\"\n\nfrom __future__ import annotations\n\ndef f(a):\n    return a + 1\n";
        let mutant = mutant_for(source, "+", "-", 1);
        let out = generated(source, &[&mutant]);
        let future = out.find("from __future__").unwrap();
        let runtime = out.find("from _angelo_rt").unwrap();
        assert!(future < runtime, "future import must stay first:\n{out}");
        assert!(out.starts_with("\"\"\"Docs.\"\"\""));
    }

    #[test]
    fn a_stale_mutant_sends_its_function_back_to_the_splice_path() {
        let source = "def f(a, b):\n    return a + b\n";
        let mut mutant = mutant_for(source, "+", "-", 1);
        mutant.original = "*".to_string();
        assert!(rewrite(source, &[&mutant]).unwrap().is_none());
    }

    /// The one invariant the whole approach rests on. Generated code that does
    /// not parse turns every mutant in the file into an `error` verdict, and an
    /// all-error run is exactly what a broken test command looks like, so this
    /// failure would be read as the wrong bug.
    #[test]
    fn everything_generated_is_valid_python() {
        let source = "\"\"\"Docs.\"\"\"\n\
             from __future__ import annotations\n\
             \n\
             LIMIT = 10 + 1\n\
             \n\
             \n\
             def top(a, b=2 * 3):\n\
             \x20   return a + b\n\
             \n\
             \n\
             async def fetch(url):\n\
             \x20   return len(url) - 1\n\
             \n\
             \n\
             def stream(n):\n\
             \x20   for i in range(n):\n\
             \x20       yield i * 2\n\
             \n\
             \n\
             class Box:\n\
             \x20   size = 3 + 4\n\
             \n\
             \x20   @property\n\
             \x20   def area(self):\n\
             \x20       return self.size * 2\n\
             \n\
             \x20   @staticmethod\n\
             \x20   def scale(x):\n\
             \x20       return x // 2\n\
             \n\
             \x20   def build(self):\n\
             \x20       def inner(y):\n\
             \x20           return y + 1\n\
             \x20       return inner\n";

        let all = mutate::enumerate_source(source, Path::new("m.py")).unwrap();
        let numbered: Vec<Mutant> = all
            .into_iter()
            .enumerate()
            .map(|(index, mut mutant)| {
                mutant.id = index as i64 + 1;
                mutant
            })
            .collect();
        let borrowed: Vec<&Mutant> = numbered.iter().collect();

        let out = generated(source, &borrowed);
        if let Err(error) = ruff_python_parser::parse_module(&out) {
            panic!("generated source does not parse: {error}\n\n{out}");
        }
        // Every shape has to be present, or this passes by generating nothing.
        assert!(out.contains("async def fetch(*_angelo_args,"));
        assert!(out.contains("return (yield from _angelo_pick"));
        assert!(out.contains("    @staticmethod\n    def scale(*_angelo_args,"));
        // Module-level and class-attribute mutants have no function to copy.
        assert!(out.contains("LIMIT = 10 + 1"));
        assert!(out.contains("    size = 3 + 4"));
    }

    /// Found on flask. `*args` mutates to `/args`, which is a syntax error.
    /// Spliced that costs one `error` verdict; compiled into the file it broke
    /// every one of the 221 mutants in the run, and the whole package with it.
    #[test]
    fn a_mutant_that_would_not_compile_is_left_to_the_splice_path() {
        let source = "def jsonify(*args, **kwargs):\n    return dict(*args) or 1 + 1\n";
        let star = mutant_for(source, "*", "/", 1);
        let plus = mutant_for(source, "+", "-", 2);

        let rewritten = rewrite(source, &[&star, &plus]).unwrap().unwrap();
        assert_eq!(rewritten.hosted, [[2]], "only the mutant that compiles");
        assert!(!rewritten.source.contains("/args"));
        ruff_python_parser::parse_module(&rewritten.source).expect("must still parse");
    }

    /// A default argument is evaluated when the copy is defined, so a mutant
    /// there runs at import time and would take the module down for everyone.
    #[test]
    fn a_mutant_in_the_signature_is_not_hosted() {
        let source = "def f(limit=10 + 1):\n    return limit\n";
        let mutant = mutant_for(source, "+", "-", 1);
        assert!(rewrite(source, &[&mutant]).unwrap().is_none());
    }

    /// Two mutants of one function are two families' worth of work and one
    /// wrapper, so a run can only ever switch one of them on. Grouping them
    /// under the same host is what tells the batch composer to keep them apart;
    /// batched together, the second silently scored `survived`, which cost 32
    /// kills on a 1000-mutant flask run.
    #[test]
    fn two_mutants_of_one_function_share_a_host() {
        let source = "def f(a, b):\n    x = a + b\n    return x * a\n";
        let plus = mutant_for(source, "+", "-", 1);
        let times = mutant_for(source, "*", "/", 2);

        let rewritten = rewrite(source, &[&plus, &times]).unwrap().unwrap();
        assert_eq!(
            rewritten.hosted,
            [[1, 2]],
            "one function, so one group, so one run each"
        );
    }

    #[test]
    fn the_active_variable_lists_every_batch_member() {
        let source = "def f(a, b):\n    return a + b\n";
        let one = mutant_for(source, "+", "-", 3);
        let two = mutant_for(source, "+", "-", 8);
        assert_eq!(Schemata::active_value(&[&one, &two]), "3,8");
        assert_eq!(Schemata::active_value(&[]), "");
    }
}
