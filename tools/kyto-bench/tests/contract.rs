//! The requirements and the contract of the rules that decide a verdict,
//! check payload digests and the tool's revision, and power the board off
//! after a failure (`docs/bench/requirements.md`, `docs/bench/contract.md`),
//! against the code and the tests: every requirement has tests in the
//! contract, a negative one among them, every test that the contract names
//! exists and runs, the rule names of the contract are those that the code
//! gives checks, refusals and reasons, and the functions, formats sections
//! and entry points that the contract names exist and say what it says.

use std::collections::{BTreeMap, BTreeSet};

const REQUIREMENTS: &str = include_str!("../../../docs/bench/requirements.md");
const CONTRACT: &str = include_str!("../../../docs/bench/contract.md");
const FORMATS: &str = include_str!("../../../docs/bench/formats.md");

/// The sources `paths`, by their paths from the repository root, with their
/// text.
macro_rules! sources {
    ($($path:literal),* $(,)?) => {
        [$(($path, include_str!(concat!("../../../", $path)))),*]
    };
}

/// Every source of the library and of the command, and the test files that
/// the contract may name. The modules that any source of the crates
/// declares are checked against this list, so a new module joins it.
const SOURCES: [(&str, &str); 42] = sources![
    "tools/kyto-bench-core/src/board.rs",
    "tools/kyto-bench-core/src/bootloader.rs",
    "tools/kyto-bench-core/src/digest.rs",
    "tools/kyto-bench-core/src/document.rs",
    "tools/kyto-bench-core/src/evidence.rs",
    "tools/kyto-bench-core/src/identity.rs",
    "tools/kyto-bench-core/src/instance.rs",
    "tools/kyto-bench-core/src/lib.rs",
    "tools/kyto-bench-core/src/lines.rs",
    "tools/kyto-bench-core/src/names.rs",
    "tools/kyto-bench-core/src/observe.rs",
    "tools/kyto-bench-core/src/pattern.rs",
    "tools/kyto-bench-core/src/payload.rs",
    "tools/kyto-bench-core/src/plan.rs",
    "tools/kyto-bench-core/src/power.rs",
    "tools/kyto-bench-core/src/public.rs",
    "tools/kyto-bench-core/src/receipt.rs",
    "tools/kyto-bench-core/src/rejudge.rs",
    "tools/kyto-bench-core/src/roles.rs",
    "tools/kyto-bench-core/src/verdict.rs",
    "tools/kyto-bench-core/src/write.rs",
    "tools/kyto-bench/src/assemble.rs",
    "tools/kyto-bench/src/cli.rs",
    "tools/kyto-bench/src/device.rs",
    "tools/kyto-bench/src/env.rs",
    "tools/kyto-bench/src/gate.rs",
    "tools/kyto-bench/src/inputs.rs",
    "tools/kyto-bench/src/lib.rs",
    "tools/kyto-bench/src/main.rs",
    "tools/kyto-bench/src/output.rs",
    "tools/kyto-bench/src/provenance.rs",
    "tools/kyto-bench/src/record.rs",
    "tools/kyto-bench/src/run.rs",
    "tools/kyto-bench/src/sim/console.rs",
    "tools/kyto-bench/src/sim/meter.rs",
    "tools/kyto-bench/src/sim/mod.rs",
    "tools/kyto-bench/src/sim/relay.rs",
    "tools/kyto-bench/src/sim/tftp.rs",
    "tools/kyto-bench/src/sim/world.rs",
    "tools/kyto-bench/tests/manifest.rs",
    "tools/kyto-bench/tests/release.rs",
    "tools/kyto-bench/tests/run.rs",
];

/// The number of requirements of each prefix, of rows of the requirement
/// tests and of rule names. A change that adds or removes a requirement, a
/// row or a rule changes these in the same change.
const REQUIREMENT_COUNTS: [(&str, usize); 3] = [("POWER", 6), ("PROV", 8), ("VERD", 12)];
const REQUIREMENT_TEST_ROWS: usize = 101;
const RULE_ROWS: usize = 32;

/// The constructors that name a rule as their first argument, called in
/// any source, with the result that they give it; that of a check is read
/// from its arguments.
const CONSTRUCTORS: [(&str, Option<&str>); 2] =
    [("Check::new(", None), ("Refusal::new(", Some("refusal"))];

/// The helpers that name a rule as their first argument, each called in
/// the one file that defines it: the file, the call, the result that the
/// helper gives the rule, and the start of its definition.
const HELPERS: [(&str, &str, &str, &str); 3] = [
    (
        "tools/kyto-bench-core/src/verdict.rs",
        "fail(",
        "fail",
        "fn fail(&mut self, rule: &str,",
    ),
    (
        "tools/kyto-bench-core/src/power.rs",
        "fault(",
        "fault",
        "fn fault(rule: &str,",
    ),
    (
        "tools/kyto-bench-core/src/payload.rs",
        "refusal(",
        "refusal",
        "let refusal = |rule, detail|",
    ),
];

/// The files whose code passes a `rule` on to a constructor of a check, a
/// refusal or a reason, each with how many times: the helpers, and
/// `evidence::eligibility`, which makes a reason of each rule of
/// `EVIDENCE_RULES` that applies. Every other file names its rules as
/// string literals.
const PASSED_ON: [(&str, usize); 4] = [
    ("tools/kyto-bench-core/src/evidence.rs", 1),
    ("tools/kyto-bench-core/src/payload.rs", 1),
    ("tools/kyto-bench-core/src/power.rs", 1),
    ("tools/kyto-bench-core/src/verdict.rs", 1),
];

/// The records that hold a rule, which code builds through their
/// constructors; written with braces, only the reasons of
/// `evidence::eligibility` pass a rule on.
const RECORDS: [&str; 3] = ["Check", "Refusal", "Reason"];

/// The starts of the names of the methods that change a `String` in place.
const CHANGES: [&str; 11] = [
    "clear",
    "drain",
    "extend",
    "insert",
    "make_ascii",
    "pop",
    "push",
    "remove",
    "replace_range",
    "retain",
    "truncate",
];

/// The source that lists the rules of evidence, `EVIDENCE_RULES`.
const EVIDENCE_SOURCE: &str = "tools/kyto-bench-core/src/receipt.rs";

/// The outcomes that a check's constructor can be given, with their
/// results.
const OUTCOMES: [(&str, &str); 3] = [
    ("Outcome::Pass", "pass"),
    ("Outcome::Fail", "fail"),
    ("Outcome::Fault", "fault"),
];

/// The code of `source` before its tests, without its comment lines.
fn code(source: &str) -> String {
    let lines = source.lines().take_while(|line| *line != "#[cfg(test)]");
    let code: Vec<&str> = lines
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    code.join("\n")
}

/// The modules that `code` declares with `mod NAME;`, whatever their
/// visibility, each with the two files that may hold it: `NAME.rs` and
/// `NAME/mod.rs` in the directory of the modules of `file`, the directory of
/// `file` itself for `lib.rs`, `main.rs` and `mod.rs`, and the directory
/// named after `file` for any other.
fn declared_modules<'a>(file: &str, code: &'a str) -> Vec<(&'a str, [String; 2])> {
    let (directory, name) = file.rsplit_once('/').unwrap_or((".", file));
    let directory = match name {
        "lib.rs" | "main.rs" | "mod.rs" => format!("{directory}/"),
        _ => format!("{directory}/{}/", name.trim_end_matches(".rs")),
    };
    let declared = code.lines().filter_map(|line| {
        let line = line.trim();
        let line = match line.strip_prefix("pub(") {
            Some(scope) => scope.split_once(')')?.1.trim_start(),
            None => line.strip_prefix("pub ").unwrap_or(line),
        };
        let module = line.strip_prefix("mod ")?.strip_suffix(';')?.trim();
        let files = [
            format!("{directory}{module}.rs"),
            format!("{directory}{module}/mod.rs"),
        ];
        Some((module, files))
    });
    declared.collect()
}

/// Whether `text` is a requirement id, two to five capital letters, `-R`
/// and two digits; its prefix and number when it is.
fn id(text: &str) -> Option<(&str, u32)> {
    let (prefix, number) = text.split_once("-R")?;
    let letters = (2..=5).contains(&prefix.len()) && prefix.bytes().all(|b| b.is_ascii_uppercase());
    let digits = number.len() == 2 && number.bytes().all(|b| b.is_ascii_digit());
    (letters && digits).then(|| (prefix, number.parse().unwrap()))
}

/// The text of the section `## {title}` of `markdown`, up to the next
/// section of that level.
fn section<'a>(markdown: &'a str, title: &str) -> Option<&'a str> {
    let heading = format!("\n## {title}\n");
    let start = markdown.find(&heading)? + heading.len();
    let rest = &markdown[start..];
    Some(&rest[..rest.find("\n## ").unwrap_or(rest.len())])
}

/// The sections of `markdown` by the anchors of their headings, as the
/// documentation check makes them: each from its heading to the next
/// heading of its level or above, outside fenced code.
fn sections(markdown: &str) -> BTreeMap<String, String> {
    let (mut fenced, mut headings) = (false, Vec::new());
    let lines: Vec<&str> = markdown.lines().collect();
    for (at, line) in lines.iter().enumerate() {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
        }
        let level = line.bytes().take_while(|b| *b == b'#').count();
        let title = line[level..].strip_prefix(' ');
        if let Some(title) = title.filter(|_| !fenced && (1..=6).contains(&level)) {
            headings.push((at, level, title));
        }
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut sections = BTreeMap::new();
    for (index, &(at, level, title)) in headings.iter().enumerate() {
        let next = headings[index + 1..].iter().find(|h| h.1 <= level);
        let text = lines[at..next.map_or(lines.len(), |h| h.0)].join("\n");
        let slug: String = title
            .trim()
            .to_lowercase()
            .chars()
            .filter_map(|c| match c {
                ' ' => Some('-'),
                c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
                _ => None,
            })
            .collect();
        let count = seen.entry(slug.clone()).or_insert(0);
        let anchor = match *count {
            0 => slug,
            n => format!("{slug}-{n}"),
        };
        *count += 1;
        sections.insert(anchor, text);
    }
    sections
}

/// The rows of the table of the section `## {title}` of `markdown`, each a
/// list of its trimmed cells, when its header is `header`.
fn table<'a>(markdown: &'a str, title: &str, header: &[&str]) -> Result<Vec<Vec<&'a str>>, String> {
    let text = section(markdown, title).ok_or_else(|| format!("contract: no section {title}"))?;
    let mut rows = text
        .lines()
        .filter(|line| line.starts_with('|'))
        .map(|line| {
            let line = line.trim().trim_start_matches('|').trim_end_matches('|');
            line.split('|').map(str::trim).collect::<Vec<_>>()
        });
    if rows.next().as_deref() != Some(header) {
        return Err(format!("contract: the table of {title} has another header"));
    }
    Ok(rows.skip(1).collect())
}

/// The code spans of `cell`, in order.
fn spans(cell: &str) -> Vec<&str> {
    cell.split('`').skip(1).step_by(2).collect()
}

/// The single code span of `cell`.
fn span(cell: &str) -> Option<&str> {
    match spans(cell)[..] {
        [one] if cell == format!("`{one}`") => Some(one),
        _ => None,
    }
}

/// The test functions of `source`, each with whether it runs: whether none
/// of its attributes ignores it or builds it only under a condition.
fn tests(source: &str) -> BTreeMap<&str, bool> {
    let (mut names, mut attributes) = (BTreeMap::new(), Vec::new());
    for line in source.lines().map(str::trim) {
        if line.starts_with("#[") {
            attributes.push(line);
            continue;
        }
        if line.starts_with("//") {
            continue;
        }
        let name = line
            .strip_prefix("fn ")
            .and_then(|line| line.split('(').next());
        if let Some(name) = name.filter(|_| attributes.contains(&"#[test]")) {
            let held = |a: &&str| a.starts_with("#[ignore") || a.starts_with("#[cfg");
            names.insert(name, !attributes.iter().any(held));
        }
        attributes.clear();
    }
    names
}

/// The arguments of a call, from after its opening parenthesis to before
/// the parenthesis that closes it.
fn arguments(after: &str) -> &str {
    let mut depth = 0usize;
    for (at, c) in after.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return &after[..at],
            ')' => depth -= 1,
            _ => {}
        }
    }
    after
}

/// Whether the text `before` a name ends inside another identifier.
fn joined(before: &str) -> bool {
    let last = before.chars().next_back();
    last.is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// The rules that `code`, the code of the file `file`, names in calls of
/// [`CONSTRUCTORS`] and of its [`HELPERS`], each with the results it gives
/// them, into `rules`; returns how many calls pass a `rule` on. A call that
/// is given another first argument than a string literal or `rule` is a
/// problem.
fn named_rules(
    file: &str,
    code: &str,
    rules: &mut BTreeMap<String, BTreeSet<String>>,
    problems: &mut Vec<String>,
) -> usize {
    let helpers = HELPERS.iter().filter(|helper| helper.0 == file);
    let helpers = helpers.map(|&(_, call, result, _)| (call, Some(result)));
    let mut passed = 0;
    for (call, result) in CONSTRUCTORS.into_iter().chain(helpers) {
        for (at, _) in code.match_indices(call) {
            let before = &code[..at];
            if joined(before) || before.ends_with("fn ") {
                continue;
            }
            let after = code[at + call.len()..].trim_start();
            let Some(literal) = after.strip_prefix('"') else {
                let rest = after.strip_prefix("rule");
                if rest.is_some_and(|rest| rest.starts_with([',', ')'])) {
                    passed += 1;
                } else {
                    problems.push(format!("{file}: {call} is given a rule that is no literal"));
                }
                continue;
            };
            let rule = &literal[..literal.find('"').unwrap_or(literal.len())];
            let given: Vec<&str> = match result {
                Some(result) => vec![result],
                None => {
                    let arguments = arguments(after);
                    let outcomes = OUTCOMES.iter().filter(|(o, _)| arguments.contains(o));
                    outcomes.map(|(_, result)| *result).collect()
                }
            };
            let [result] = given[..] else {
                problems.push(format!("{file}: the result of {rule} is not one outcome"));
                continue;
            };
            let results = rules.entry(rule.to_string()).or_default();
            results.insert(result.to_string());
        }
    }
    passed
}

/// How many [`RECORDS`] `code`, the code of the file `file`, builds with
/// braces from a `rule` that it passes on. Any other record built with
/// braces is a problem.
fn built_records(file: &str, code: &str, problems: &mut Vec<String>) -> usize {
    let mut passed = 0;
    for record in RECORDS {
        let open = format!("{record} {{");
        for (at, _) in code.match_indices(&open) {
            let before = &code[..at];
            let words = ["struct ", "enum ", "trait ", "impl ", "for ", "-> "];
            if joined(before) || words.iter().any(|word| before.ends_with(word)) {
                continue;
            }
            let fields = code[at + open.len()..].trim_start();
            if fields.starts_with("rule,") || fields.starts_with("rule }") {
                passed += 1;
            } else {
                problems.push(format!("{file}: a {record} is built with braces"));
            }
        }
    }
    passed
}

/// The name of the function that `line` of code defines, if any.
fn defined(line: &str) -> Option<&str> {
    let at = line.find("fn ")?;
    let starts = at == 0 || line[..at].ends_with(' ');
    let name = line[at + "fn ".len()..].split(['(', '<']).next()?.trim();
    starts.then_some(name)
}

/// The other ways in which `code`, the code of the file `file`, could give
/// a rule, each a problem: a function of one of [`RECORDS`], in any of its
/// `impl` blocks, other than its `new`; such a block that builds a record of
/// `Self` outside `new`; and a `rule` field that is assigned, changed in
/// place or borrowed to be changed.
fn other_ways(file: &str, code: &str, problems: &mut Vec<String>) {
    let lines: Vec<&str> = code.lines().collect();
    for (at, line) in lines.iter().enumerate() {
        let header = line.trim_start();
        if !(header.starts_with("impl ") || header.starts_with("impl<")) {
            continue;
        }
        let header = header.split('{').next().unwrap_or(header);
        let mut words = header.split(|c: char| !(c.is_alphanumeric() || c == '_'));
        let target = words.rfind(|word| !word.is_empty()).unwrap_or("");
        let Some(record) = RECORDS.iter().find(|record| **record == target) else {
            continue;
        };
        let indent = line.len() - line.trim_start().len();
        let close = format!("{}}}", " ".repeat(indent));
        let end = lines[at..].iter().position(|line| *line == close);
        let block = &lines[at..end.map_or(lines.len(), |end| at + end + 1)];
        let new = function(block, 0, "new").unwrap_or_default();
        let rest = block.join("\n").replacen(&new, "", 1);
        for name in rest.lines().filter_map(defined) {
            problems.push(format!("{file}: {record} has the function {name}"));
        }
        if rest.contains("Self::new(") || rest.contains("Self {") {
            problems.push(format!("{file}: a {record} is built outside its new"));
        }
    }
    for (at, _) in code.match_indices(".rule") {
        let after = &code[at + ".rule".len()..];
        if after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let after = after.trim_start();
        let assigned = after.starts_with('=') && !after.starts_with("==");
        let changed = after
            .strip_prefix('.')
            .is_some_and(|call| CHANGES.iter().any(|change| call.starts_with(change)));
        let owner = code[..at].trim_end_matches(|c: char| c.is_alphanumeric() || "_.".contains(c));
        if assigned || changed || after.starts_with("+=") || owner.ends_with("&mut ") {
            problems.push(format!("{file}: a rule is assigned or changed"));
        }
    }
}

/// The rules of `EVIDENCE_RULES` in `source`.
fn evidence_rules(source: &str) -> Vec<String> {
    let code = code(source);
    let list = code
        .split_once("EVIDENCE_RULES: [&str; ")
        .and_then(|(_, rest)| rest.split_once("= ["))
        .and_then(|(_, rest)| rest.split_once("];"))
        .map_or("", |(list, _)| list);
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(String::from)
        .collect()
}

/// The line of `lines`, from `from` on, that defines the function `name`,
/// with the lines up to the brace that closes it at its indentation.
fn function(lines: &[&str], from: usize, name: &str) -> Option<String> {
    let defines = |line: &&str| {
        let at = [format!("fn {name}("), format!("fn {name}<")]
            .iter()
            .find_map(|text| line.find(text.as_str()));
        at.is_some_and(|at| at == 0 || line[..at].ends_with(' '))
    };
    let start = from + lines[from..].iter().position(defines)?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines[start..].iter().position(|line| *line == close);
    let end = end.map_or(lines.len(), |end| start + end + 1);
    Some(lines[start..end].join("\n"))
}

/// The source file and the code of the item that `path`, such as
/// `verdict::Judging::markers`, names, when it is defined: a function, from
/// its definition to the brace that closes it, or the line that declares a
/// type.
fn item<'a>(sources: &[(&'a str, &str)], path: &str) -> Option<(&'a str, String)> {
    let segments: Vec<&str> = path.split("::").collect();
    let word = |line: &str, name: &str| {
        let mut words = line.split(|c: char| !(c.is_alphanumeric() || c == '_'));
        words.any(|word| word == name)
    };
    (1..segments.len()).rev().find_map(|split| {
        let module = format!("/src/{}.rs", segments[..split].join("/"));
        let (file, text) = sources.iter().find(|(file, _)| file.ends_with(&module))?;
        let code = code(text);
        let lines: Vec<&str> = code.lines().collect();
        let found = match segments[split..] {
            [name] => function(&lines, 0, name).or_else(|| {
                let kinds = ["struct ", "enum ", "trait "];
                let declares = |line: &&&str| {
                    let at = kinds.iter().find_map(|kind| line.find(kind));
                    at.is_some_and(|at| word(&line[at..], name))
                };
                lines.iter().find(declares).map(|line| line.to_string())
            }),
            [owner, name] => {
                let owns = |line: &&str| {
                    let opens = line.starts_with("impl") || line.contains("trait ");
                    opens && word(line, owner)
                };
                let from = lines.iter().position(owns)?;
                function(&lines, from, name)
            }
            _ => None,
        };
        Some((*file, found?))
    })
}

/// Every way in which `requirements`, `contract` and `formats` disagree
/// with each other and with `sources`, the sources by their paths.
fn problems(
    requirements: &str,
    contract: &str,
    formats: &str,
    sources: &[(&str, &str)],
) -> Vec<String> {
    let mut problems = Vec::new();
    let source = |file: &str| sources.iter().find(|(path, _)| *path == file).map(|s| s.1);

    // Every module that a source of the crates declares, in any of them and
    // whatever its visibility, is a source of this test.
    for (file, text) in sources.iter().filter(|(file, _)| file.contains("/src/")) {
        let code = code(text);
        for (module, files) in declared_modules(file, &code) {
            if !files.iter().any(|file| source(file).is_some()) {
                problems.push(format!(
                    "{file}: the module {module} is not a source of this test"
                ));
            }
        }
    }

    // The requirement ids, from R01 in order in each prefix, as many as the
    // test expects.
    let mut ids = BTreeSet::new();
    let mut numbers: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    for heading in requirements.lines().filter_map(|l| l.strip_prefix("### ")) {
        match id(heading) {
            Some((prefix, number)) => numbers.entry(prefix).or_default().push(number),
            None => problems.push(format!("requirements: {heading} is not a requirement id")),
        }
        if !ids.insert(heading) {
            problems.push(format!("requirements: {heading} comes twice"));
        }
    }
    for (prefix, found) in &numbers {
        let expected: Vec<u32> = (1..=found.len() as u32).collect();
        if *found != expected {
            problems.push(format!(
                "requirements: {prefix} runs {found:?}, not from R01 in order"
            ));
        }
        if !REQUIREMENT_COUNTS.iter().any(|(known, _)| known == prefix) {
            problems.push(format!("requirements: {prefix} is not a known prefix"));
        }
    }
    for (prefix, expected) in REQUIREMENT_COUNTS {
        let found = numbers.get(prefix).map_or(0, Vec::len);
        if found != expected {
            problems.push(format!(
                "requirements: {prefix} has {found} requirements, not {expected}"
            ));
        }
    }

    // The requirement tests: a known test that runs, of a known file, for
    // every row, and a negative one for every requirement.
    let header = ["Requirement", "Test", "File", "Kind"];
    let rows = table(contract, "Requirement tests", &header).unwrap_or_else(|problem| {
        problems.push(problem);
        Vec::new()
    });
    if rows.len() != REQUIREMENT_TEST_ROWS {
        problems.push(format!(
            "contract: the requirement tests have {} rows, not {REQUIREMENT_TEST_ROWS}",
            rows.len()
        ));
    }
    let (mut tested, mut negative, mut pairs) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    for row in &rows {
        let [requirement, test, file, kind] = row[..] else {
            problems.push(format!(
                "contract: the row {row:?} has another number of cells"
            ));
            continue;
        };
        let requirement = requirement
            .strip_prefix('[')
            .and_then(|cell| cell.split_once("](requirements.md#"))
            .filter(|(id, anchor)| anchor.strip_suffix(')') == Some(&id.to_lowercase()))
            .map(|(id, _)| id);
        let (Some(requirement), Some(test), Some(file)) = (requirement, span(test), span(file))
        else {
            problems.push(format!(
                "contract: the row {row:?} is not a link and two code spans"
            ));
            continue;
        };
        if !ids.contains(requirement) {
            problems.push(format!("contract: {requirement} is not a requirement"));
        }
        tested.insert(requirement);
        match kind {
            "negative" => {
                negative.insert(requirement);
            }
            "positive" => {}
            _ => problems.push(format!("contract: {kind} is not a kind of test")),
        }
        if !pairs.insert((requirement, test, file)) {
            problems.push(format!(
                "contract: {requirement} names {test} of {file} twice"
            ));
        }
        match source(file).map(tests) {
            None => problems.push(format!("contract: {file} is not a source of this test")),
            Some(found) => match found.get(test) {
                None => problems.push(format!("contract: {file} has no test {test}")),
                Some(false) => problems.push(format!("contract: {test} of {file} does not run")),
                Some(true) => {}
            },
        }
    }
    for id in &ids {
        if !tested.contains(id) {
            problems.push(format!("contract: {id} has no test"));
        } else if !negative.contains(id) {
            problems.push(format!("contract: {id} has no negative test"));
        }
    }

    // The rule names: those of the code, with their results, their
    // requirements, the functions that name them and the sections of the
    // formats page that give them.
    let mut named = BTreeMap::new();
    for (file, text) in sources {
        if !file.contains("/src/") {
            continue;
        }
        let code = code(text);
        let passed = named_rules(file, &code, &mut named, &mut problems)
            + built_records(file, &code, &mut problems);
        other_ways(file, &code, &mut problems);
        let expected = PASSED_ON.iter().find(|(f, _)| f == file).map_or(0, |p| p.1);
        if passed != expected {
            problems.push(format!(
                "{file} passes a rule on {passed} times, not {expected}"
            ));
        }
    }
    for (file, _, _, definition) in HELPERS {
        if !code(source(file).unwrap_or("")).contains(definition) {
            problems.push(format!("{file} has no helper {definition}"));
        }
    }
    let evidence = evidence_rules(source(EVIDENCE_SOURCE).unwrap_or(""));
    for rule in &evidence {
        named
            .entry(rule.clone())
            .or_default()
            .insert("reason".to_string());
    }
    let header = ["Rule", "Group", "Result", "Function", "Formats"];
    let rows = table(contract, "Rule names", &header).unwrap_or_else(|problem| {
        problems.push(problem);
        Vec::new()
    });
    if rows.len() != RULE_ROWS {
        problems.push(format!(
            "contract: the rule names have {} rows, not {RULE_ROWS}",
            rows.len()
        ));
    }
    let sections = sections(formats);
    let mut listed = BTreeSet::new();
    for row in &rows {
        let [rule, group, result, functions, link] = row[..] else {
            problems.push(format!(
                "contract: the row {row:?} has another number of cells"
            ));
            continue;
        };
        let Some(rule) = span(rule) else {
            problems.push(format!("contract: the rule {rule} is not one code span"));
            continue;
        };
        if !listed.insert(rule) {
            problems.push(format!("contract: the rule {rule} is listed twice"));
        }
        match named.get(rule) {
            None => problems.push(format!("contract: the code does not name the rule {rule}")),
            Some(results) if results.iter().ne([result]) => {
                problems.push(format!(
                    "contract: the code gives {rule} {results:?}, not {result}"
                ));
            }
            Some(_) => {}
        }
        let groups = spans(group);
        if groups.is_empty() || !groups.iter().all(|group| ids.contains(group)) {
            problems.push(format!("contract: the group of {rule} is not requirements"));
        }
        let functions = spans(functions);
        if functions.is_empty() {
            problems.push(format!("contract: {rule} has no function"));
        }
        let quoted = format!("\"{rule}\"");
        let reads_evidence = |body: &str| {
            evidence.iter().any(|known| known == rule) && body.contains("EVIDENCE_RULES")
        };
        for function in functions {
            match item(sources, function) {
                None => problems.push(format!(
                    "contract: the function {function} of {rule} is not defined"
                )),
                Some((_, body)) if !body.contains(&quoted) && !reads_evidence(&body) => {
                    problems.push(format!(
                        "contract: the function {function} does not name {rule}"
                    ));
                }
                Some(_) => {}
            }
        }
        let anchor = link
            .split_once("](formats.md#")
            .and_then(|(_, rest)| rest.strip_suffix(')'));
        match anchor.and_then(|anchor| Some((anchor, sections.get(anchor)?))) {
            None => problems.push(format!(
                "contract: the link of {rule} is not a heading of the formats page"
            )),
            Some((anchor, text)) if !text.contains(&format!("`{rule}`")) => {
                problems.push(format!(
                    "formats: the section {anchor} does not name the rule {rule}"
                ));
            }
            Some(_) => {}
        }
    }
    for rule in named.keys() {
        if !listed.contains(rule.as_str()) {
            problems.push(format!("contract: the rule names lack {rule}"));
        }
    }

    // The entry points: functions and types of their modules and crates,
    // called by functions that exist, for requirements that exist.
    let header = ["Entry point", "Crate", "Called by", "Requirements"];
    let rows = table(contract, "Entry points", &header).unwrap_or_else(|problem| {
        problems.push(problem);
        Vec::new()
    });
    for row in &rows {
        let [entries, krate, callers, requirements] = row[..] else {
            problems.push(format!(
                "contract: the row {row:?} has another number of cells"
            ));
            continue;
        };
        let krate = span(krate).unwrap_or(krate);
        for entry in spans(entries) {
            match item(sources, entry) {
                None => problems.push(format!("contract: the entry point {entry} is not defined")),
                Some((file, _)) if !file.starts_with(&format!("tools/{krate}/")) => {
                    problems.push(format!(
                        "contract: the entry point {entry} is not of {krate}"
                    ));
                }
                Some(_) => {}
            }
        }
        for caller in spans(callers) {
            if item(sources, caller).is_none() {
                problems.push(format!("contract: the caller {caller} is not defined"));
            }
        }
        let requirements = spans(requirements);
        if requirements.is_empty() || !requirements.iter().all(|id| ids.contains(id)) {
            problems.push(format!(
                "contract: the requirements of {entries} are not requirements"
            ));
        }
    }
    problems
}

/// Asserts that one of `problems` contains `found`.
#[track_caller]
fn assert_finds(problems: &[String], found: &str) {
    let any = problems.iter().any(|problem| problem.contains(found));
    assert!(any, "{found:?} is not among {problems:#?}");
}

/// The problems of the pages and the sources when `edit` gives the text of
/// the source `file`, and the sources `added` join them.
fn edited(file: &str, edit: impl Fn(&str) -> String, added: &[(&str, &str)]) -> Vec<String> {
    let text = SOURCES.iter().find(|(path, _)| *path == file).unwrap().1;
    let edited = edit(text);
    let pick = |&(path, text): &(&'static str, &'static str)| {
        (path, if path == file { edited.as_str() } else { text })
    };
    let mut sources: Vec<(&str, &str)> = SOURCES.iter().map(pick).collect();
    sources.extend_from_slice(added);
    problems(REQUIREMENTS, CONTRACT, FORMATS, &sources)
}

#[test]
fn the_contract_lists_every_requirement_and_rule() {
    let none: Vec<String> = Vec::new();
    assert_eq!(problems(REQUIREMENTS, CONTRACT, FORMATS, &SOURCES), none);
    // Each check finds what it looks for, in an edited copy of a page or a
    // source.
    let requirements = |from: &str, to: &str| {
        assert!(REQUIREMENTS.contains(from), "{from}");
        let edited = REQUIREMENTS.replacen(from, to, 1);
        problems(&edited, CONTRACT, FORMATS, &SOURCES)
    };
    let contract = |from: &str, to: &str| {
        assert!(CONTRACT.contains(from), "{from}");
        let edited = CONTRACT.replacen(from, to, 1);
        problems(REQUIREMENTS, &edited, FORMATS, &SOURCES)
    };
    // A source with its text `from` changed to `to`, and one with an item
    // put before its text: the checks read every item of the code wherever
    // it stands.
    let source = |file: &str, from: &str, to: &str| {
        let edit = |text: &str| {
            assert!(text.contains(from), "{from}");
            text.replacen(from, to, 1)
        };
        edited(file, edit, &[])
    };
    let prepend = |file: &str, item: &str| edited(file, |text| format!("{item}{text}"), &[]);
    // A heading that is not an id, ids with a gap or out of order, and a
    // prefix with a requirement more or less, its last one removed with its
    // rows.
    let found = requirements("### VERD-R12", "### VERD-R1");
    assert_finds(&found, "VERD-R1 is not a requirement id");
    let found = requirements("### VERD-R12", "### VERD-R13");
    assert_finds(&found, "VERD runs");
    assert_finds(&requirements("### PROV-R02", "### PROV-R03"), "PROV runs");
    let found = requirements("### POWER-R06", "### power-R06");
    assert_finds(&found, "power-R06 is not a requirement id");
    let found = requirements("### POWER-R06", "### POWER-R06\n\n### POWER-R07");
    assert_finds(&found, "POWER has 7 requirements, not 6");
    assert_finds(&found, "POWER-R07 has no test");
    let found = requirements("### VERD-R01", "### VER-R01");
    assert_finds(&found, "VER is not a known prefix");
    let last = REQUIREMENTS.find("### POWER-R06").unwrap();
    let rest = REQUIREMENTS[last + 1..]
        .find("\n#")
        .map_or(REQUIREMENTS.len(), |at| last + at + 2);
    let fewer = format!("{}{}", &REQUIREMENTS[..last], &REQUIREMENTS[rest..]);
    let lines = CONTRACT
        .lines()
        .filter(|line| !line.starts_with("| [POWER-R06]"));
    let without = lines.collect::<Vec<_>>().join("\n");
    let found = problems(&fewer, &without, FORMATS, &SOURCES);
    assert_finds(&found, "POWER has 5 requirements, not 6");
    // A requirement without a row or without a negative one, a row removed,
    // and a row of another requirement, anchor, test, file or kind.
    let row = "| [POWER-R05](requirements.md#power-r05) \
        | `a_run_that_panics_switches_the_supply_off_and_leaves_no_output` \
        | `tools/kyto-bench/tests/run.rs` | negative |\n";
    let edit = |from: &str, to: &str| contract(row, &row.replace(from, to));
    assert_finds(&contract(row, ""), "POWER-R05 has no test");
    assert_finds(
        &edit("negative", "positive"),
        "POWER-R05 has no negative test",
    );
    let other = "| [VERD-R03](requirements.md#verd-r03) \
        | `a_receipt_has_either_a_refusal_or_a_verdict` \
        | `tools/kyto-bench-core/src/receipt.rs` | negative |\n";
    let fewer = REQUIREMENT_TEST_ROWS - 1;
    let rows = format!("the requirement tests have {fewer} rows, not {REQUIREMENT_TEST_ROWS}");
    assert_finds(&contract(other, ""), &rows);
    let found = edit(
        "POWER-R05](requirements.md#power-r05",
        "POWER-R07](requirements.md#power-r07",
    );
    assert_finds(&found, "POWER-R07 is not a requirement");
    assert_finds(&edit("#power-r05", "#power-r04"), "is not a link");
    let found = edit("leaves_no_output", "leaves_nothing");
    assert_finds(&found, "run.rs has no test a_run_that_panics");
    let found = edit("tests/run.rs", "tests/release.rs");
    assert_finds(&found, "release.rs has no test a_run_that_panics");
    let found = edit("tests/run.rs", "tests/data.rs");
    assert_finds(&found, "data.rs is not a source of this test");
    assert_finds(&edit("negative", "fuzz"), "fuzz is not a kind of test");
    assert_finds(
        &contract(row, &format!("{row}{row}")),
        "names a_run_that_panics",
    );
    // A test that is not one, and tests that do not run.
    let run = "tools/kyto-bench/tests/run.rs";
    let test = "#[test]\nfn a_run_that_panics";
    let found = source(run, test, "fn a_run_that_panics");
    assert_finds(&found, "run.rs has no test a_run_that_panics");
    let found = source(run, test, "#[test]\n#[ignore]\nfn a_run_that_panics");
    assert_finds(
        &found,
        "and_leaves_no_output of tools/kyto-bench/tests/run.rs does not run",
    );
    let found = source(
        run,
        test,
        "#[ignore = \"slow\"]\n#[test]\nfn a_run_that_panics",
    );
    assert_finds(
        &found,
        "and_leaves_no_output of tools/kyto-bench/tests/run.rs does not run",
    );
    let found = source(run, test, "#[cfg(any())]\n#[test]\nfn a_run_that_panics");
    assert_finds(
        &found,
        "and_leaves_no_output of tools/kyto-bench/tests/run.rs does not run",
    );
    // A rule of the code that the table lacks, or lists with another result,
    // group or function; a rule that the code or the formats page does not
    // name; a rule listed twice; and a rule row removed.
    let rule = "| `device.error` | `POWER-R03` | fault | `power::device_errors` \
        | [checks](formats.md#checks) |\n";
    let edit = |from: &str, to: &str| contract(rule, &rule.replace(from, to));
    let found = contract(rule, "");
    assert_finds(&found, "the rule names lack device.error");
    assert_finds(&found, "the rule names have 31 rows, not 32");
    assert_finds(
        &edit("| fault |", "| fail |"),
        "the code gives device.error",
    );
    assert_finds(&edit("POWER-R03", "POWER-R09"), "the group of device.error");
    let found = edit("device_errors", "device_faults");
    assert_finds(&found, "the function power::device_faults of device.error");
    let found = edit("power::device_errors", "verdict::device_errors");
    assert_finds(
        &found,
        "the function verdict::device_errors of device.error",
    );
    let found = edit("power::device_errors", "power::cold_boot_checks");
    assert_finds(&found, "power::cold_boot_checks does not name device.error");
    let found = edit("device.error", "device.failure");
    assert_finds(&found, "the code does not name the rule device.failure");
    assert_finds(
        &found,
        "the section checks does not name the rule device.failure",
    );
    let found = contract(rule, &format!("{rule}{rule}"));
    assert_finds(&found, "the rule device.error is listed twice");
    // A function that does not name its rule, and a link to a section that
    // is not one or that does not name the rule.
    let rule = "| `boot.end` | `VERD-R06` | fail | `verdict::Judging::boot_end` \
        | [checks](formats.md#checks) |\n";
    let found = contract(rule, &rule.replace("boot_end", "markers"));
    assert_finds(&found, "verdict::Judging::markers does not name boot.end");
    let found = contract(rule, &rule.replace("#checks", "#no-such-section"));
    assert_finds(&found, "the link of boot.end is not a heading");
    let found = contract(rule, &rule.replace("#checks", "#the-supply"));
    assert_finds(
        &found,
        "the section the-supply does not name the rule boot.end",
    );
    let formats = FORMATS.replace("`identity.disagree`", "`identity.differ`");
    let found = problems(REQUIREMENTS, CONTRACT, &formats, &SOURCES);
    assert_finds(
        &found,
        "the section checks does not name the rule identity.disagree",
    );
    // A rule that the code names under another name or with another result,
    // in a module that names no other, through a helper that the test does
    // not know or with braces; a rule that is not a literal; a helper that
    // is gone; and a rule of evidence that is gone or not read.
    let verdict = "tools/kyto-bench-core/src/verdict.rs";
    let found = source(verdict, "\"identity.disagree\"", "\"identity.split\"");
    assert_finds(&found, "the rule names lack identity.split");
    assert_finds(&found, "the code does not name the rule identity.disagree");
    let payload = "tools/kyto-bench-core/src/payload.rs";
    let found = source(payload, "Outcome::Fault,\n", "Outcome::Fail,\n");
    assert_finds(&found, "the code gives payload.served_digest");
    let rejudge = "tools/kyto-bench-core/src/rejudge.rs";
    let extra = "fn extra() -> Check {\n    Check::new(\"rejudge.extra\", \"x\", Outcome::Fault, None)\n}\n\n";
    assert_finds(
        &prepend(rejudge, extra),
        "the rule names lack rejudge.extra",
    );
    let flag =
        "fn flag(rule: &str) -> Check {\n    Check::new(rule, \"x\", Outcome::Fail, None)\n}\n\n";
    let found = prepend(verdict, flag);
    assert_finds(&found, "verdict.rs passes a rule on 2 times, not 1");
    let flag = "fn flag(judging: &mut Judging, rule: &str) {\n    \
        judging.fail(rule, \"x\", None);\n}\n\n";
    let found = prepend(verdict, flag);
    assert_finds(&found, "verdict.rs passes a rule on 2 times, not 1");
    let assemble = "tools/kyto-bench/src/assemble.rs";
    let forged =
        "fn forged() -> Reason {\n    Reason { detail: String::new(), rule: \"x\".into() }\n}\n\n";
    let found = prepend(assemble, forged);
    assert_finds(&found, "assemble.rs: a Reason is built with braces");
    let forged =
        "fn forged(rule: String) -> Reason {\n    Reason { rule, detail: String::new() }\n}\n\n";
    let found = prepend(assemble, forged);
    assert_finds(&found, "assemble.rs passes a rule on 1 times, not 0");
    let power = "tools/kyto-bench-core/src/power.rs";
    let found = source(power, "fault(\"power.order\"", "fault(order_rule");
    assert_finds(
        &found,
        "power.rs: fault( is given a rule that is no literal",
    );
    let found = source(power, "fn fault(rule: &str,", "fn flaw(rule: &str,");
    assert_finds(&found, "power.rs has no helper fn fault(rule: &str,");
    let receipt = "tools/kyto-bench-core/src/receipt.rs";
    let found = source(receipt, "\"evidence.simulated\",\n", "");
    assert_finds(&found, "the code does not name the rule evidence.simulated");
    let evidence = "tools/kyto-bench-core/src/evidence.rs";
    let found = source(evidence, "= EVIDENCE_RULES\n", "= RULES\n");
    assert_finds(
        &found,
        "evidence::eligibility does not name evidence.simulated",
    );
    // A function of a record other than its `new`, in its own `impl` block
    // or in that of a trait, a record built from `Self` outside its `new`,
    // and a rule that is assigned, changed in place or borrowed to be
    // changed.
    let failed = "impl Check {\n    pub fn failed(rule: &str, subject: &str) -> Self {\n        \
        Self::new(rule, subject, Outcome::Fail, None)\n    }\n}\n\n";
    let found = prepend(receipt, failed);
    assert_finds(&found, "receipt.rs: Check has the function failed");
    assert_finds(&found, "receipt.rs: a Check is built outside its new");
    let named = "impl From<&str> for Refusal {\n    fn from(rule: &str) -> Self {\n        \
        Self { rule: rule.into(), entry: None, subject: String::new(), detail: String::new() }\n    \
        }\n}\n\n";
    let found = prepend(receipt, named);
    assert_finds(&found, "receipt.rs: Refusal has the function from");
    assert_finds(&found, "receipt.rs: a Refusal is built outside its new");
    let blank = "impl Reason {\n    const BLANK: Self = Self {\n        rule: String::new(),\n        \
        detail: String::new(),\n    };\n}\n\n";
    let found = prepend(receipt, blank);
    assert_finds(&found, "receipt.rs: a Reason is built outside its new");
    for change in [
        "check.rule = \"x\".into();",
        "check.rule += \"x\";",
        "check.rule.push_str(\"x\");",
        "std::mem::take(&mut check.rule);",
    ] {
        let rename = format!("fn rename(check: &mut Check) {{\n    {change}\n}}\n\n");
        let found = prepend(rejudge, &rename);
        assert_finds(&found, "rejudge.rs: a rule is assigned or changed");
    }
    // A module declared in a crate root or in any other source, whatever its
    // visibility, and one whose file is a source of the test.
    let core = "tools/kyto-bench-core/src/lib.rs";
    for module in [
        "pub mod extra;\n",
        "pub(crate) mod extra;\n",
        "mod extra;\n",
    ] {
        assert_finds(
            &prepend(core, module),
            "kyto-bench-core/src/lib.rs: the module extra is not a source",
        );
    }
    let sim = "tools/kyto-bench/src/sim/mod.rs";
    let found = prepend(sim, "pub(in crate::sim) mod extra;\n");
    assert_finds(&found, "sim/mod.rs: the module extra is not a source");
    let found = prepend(verdict, "mod helpers;\n");
    assert_finds(&found, "verdict.rs: the module helpers is not a source");
    let found = prepend(power, "pub(super) mod helpers;\n");
    assert_finds(&found, "power.rs: the module helpers is not a source");
    let helpers = [("tools/kyto-bench-core/src/verdict/helpers.rs", "")];
    let found = edited(verdict, |text| format!("mod helpers;\n{text}"), &helpers);
    assert_eq!(found, none);
    let helpers = [("tools/kyto-bench/src/sim/helpers/mod.rs", "")];
    let found = edited(sim, |text| format!("mod helpers;\n{text}"), &helpers);
    assert_eq!(found, none);
    // An entry point or a caller that is not defined or is of another
    // crate, and a requirement there that is not one.
    let found = contract("| `run::check_sources`;", "| `run::check_source`;");
    assert_finds(&found, "the caller run::check_source is not defined");
    let row = "| `payload::check_staged` | `kyto-bench-core` |";
    let found = contract(row, "| `payload::check_stage` | `kyto-bench-core` |");
    assert_finds(
        &found,
        "the entry point payload::check_stage is not defined",
    );
    let found = contract(row, "| `payload::check_staged` | `kyto-bench` |");
    assert_finds(&found, "payload::check_staged is not of kyto-bench");
    let found = contract(
        "| `PROV-R02`, `PROV-R04` |\n",
        "| `PROV-R02`, `PROV-R09` |\n",
    );
    assert_finds(&found, "the requirements of `payload::check_staged`");
}

#[test]
fn the_contract_check_finds_ids_tables_and_cells_out_of_form() {
    let requirements = |from: &str, to: &str| {
        assert!(REQUIREMENTS.contains(from), "{from}");
        let edited = REQUIREMENTS.replacen(from, to, 1);
        problems(&edited, CONTRACT, FORMATS, &SOURCES)
    };
    let contract = |from: &str, to: &str| {
        assert!(CONTRACT.contains(from), "{from}");
        let edited = CONTRACT.replacen(from, to, 1);
        problems(REQUIREMENTS, &edited, FORMATS, &SOURCES)
    };
    // Prefixes of one and of six letters, a number of three digits, and a
    // requirement given twice.
    for id in ["V-R01", "VERDIC-R01", "VERD-R001"] {
        let found = requirements("### VERD-R01\n", &format!("### {id}\n"));
        assert_finds(&found, &format!("{id} is not a requirement id"));
    }
    let found = requirements("### POWER-R06", "### POWER-R06\n\n### POWER-R06");
    assert_finds(&found, "POWER-R06 comes twice");
    // A table with another header, and a test cell with more than its code
    // span.
    let header = "| Requirement | Test | File | Kind |";
    let found = contract(header, "| Requirement | Test | Path | Kind |");
    assert_finds(&found, "the table of Requirement tests has another header");
    let test = "`a_run_that_panics_switches_the_supply_off_and_leaves_no_output`";
    let found = contract(test, &format!("{test} (run)"));
    assert_finds(&found, "is not a link and two code spans");
    // A rule of two groups, one of them no requirement, a rule without a
    // function, and a rule that the code gives two results.
    let rule =
        "| `payload.source_digest` | `PROV-R01`, `PROV-R04` | refusal | `payload::check_entry` |";
    let found = contract(rule, &rule.replace("PROV-R04", "PROV-R09"));
    assert_finds(
        &found,
        "the group of payload.source_digest is not requirements",
    );
    let found = contract(rule, &rule.replace("`payload::check_entry`", "check_entry"));
    assert_finds(&found, "payload.source_digest has no function");
    let rejudge = "tools/kyto-bench-core/src/rejudge.rs";
    let other = "fn other() -> Check {\n    Check::new(\"device.error\", \"x\", Outcome::Fail, None)\n}\n\n";
    let found = edited(rejudge, |text| format!("{other}{text}"), &[]);
    assert_finds(
        &found,
        "the code gives device.error {\"fail\", \"fault\"}, not fault",
    );
}
