//! The requirements and the contract of the rules that decide a verdict,
//! check payload digests and the tool's revision, and power the board off
//! after a failure (`docs/bench/requirements.md`, `docs/bench/contract.md`),
//! against the code and the tests: every requirement has tests in the
//! contract, a negative one among them, every test that the contract names
//! exists, and the rule names of the contract are those that the code gives
//! checks, refusals and reasons.

use std::collections::{BTreeMap, BTreeSet};

const REQUIREMENTS: &str = include_str!("../../../docs/bench/requirements.md");
const CONTRACT: &str = include_str!("../../../docs/bench/contract.md");
const FORMATS: &str = include_str!("../../../docs/bench/formats.md");

/// The sources that the contract may name, by their paths from the
/// repository root: those of the tests it names and of the modules that
/// name rules.
const SOURCES: [(&str, &str); 15] = [
    (
        "tools/kyto-bench-core/src/evidence.rs",
        include_str!("../../kyto-bench-core/src/evidence.rs"),
    ),
    (
        "tools/kyto-bench-core/src/lines.rs",
        include_str!("../../kyto-bench-core/src/lines.rs"),
    ),
    (
        "tools/kyto-bench-core/src/observe.rs",
        include_str!("../../kyto-bench-core/src/observe.rs"),
    ),
    (
        "tools/kyto-bench-core/src/payload.rs",
        include_str!("../../kyto-bench-core/src/payload.rs"),
    ),
    (
        "tools/kyto-bench-core/src/power.rs",
        include_str!("../../kyto-bench-core/src/power.rs"),
    ),
    (
        "tools/kyto-bench-core/src/receipt.rs",
        include_str!("../../kyto-bench-core/src/receipt.rs"),
    ),
    (
        "tools/kyto-bench-core/src/rejudge.rs",
        include_str!("../../kyto-bench-core/src/rejudge.rs"),
    ),
    (
        "tools/kyto-bench-core/src/verdict.rs",
        include_str!("../../kyto-bench-core/src/verdict.rs"),
    ),
    ("tools/kyto-bench/src/cli.rs", include_str!("../src/cli.rs")),
    (
        "tools/kyto-bench/src/gate.rs",
        include_str!("../src/gate.rs"),
    ),
    (
        "tools/kyto-bench/src/provenance.rs",
        include_str!("../src/provenance.rs"),
    ),
    (
        "tools/kyto-bench/src/sim/meter.rs",
        include_str!("../src/sim/meter.rs"),
    ),
    (
        "tools/kyto-bench/src/sim/tftp.rs",
        include_str!("../src/sim/tftp.rs"),
    ),
    (
        "tools/kyto-bench/tests/release.rs",
        include_str!("release.rs"),
    ),
    ("tools/kyto-bench/tests/run.rs", include_str!("run.rs")),
];

/// The modules that name rules, by the first segment of a function of the
/// rule names table, with their sources.
const MODULES: [(&str, &str); 4] = [
    ("evidence", "tools/kyto-bench-core/src/evidence.rs"),
    ("payload", "tools/kyto-bench-core/src/payload.rs"),
    ("power", "tools/kyto-bench-core/src/power.rs"),
    ("verdict", "tools/kyto-bench-core/src/verdict.rs"),
];

/// The sources whose code gives checks and refusals their rule names.
const RULE_SOURCES: [&str; 3] = [
    "tools/kyto-bench-core/src/payload.rs",
    "tools/kyto-bench-core/src/power.rs",
    "tools/kyto-bench-core/src/verdict.rs",
];

/// The source that lists the rules of evidence, `EVIDENCE_RULES`.
const EVIDENCE_SOURCE: &str = "tools/kyto-bench-core/src/receipt.rs";

/// The calls that name a rule as their first argument, with the result that
/// they give it: the helpers that make a failed check, a fault and a
/// refusal, and the constructors of a refusal and of a check, whose result
/// is then read from its arguments.
const CALLS: [(&str, Option<&str>); 5] = [
    ("fail(", Some("fail")),
    ("fault(", Some("fault")),
    ("refusal(", Some("refusal")),
    ("Refusal::new(", Some("refusal")),
    ("Check::new(", None),
];

/// The outcomes that a check's constructor can be given, with their
/// results.
const OUTCOMES: [(&str, &str); 3] = [
    ("Outcome::Pass", "pass"),
    ("Outcome::Fail", "fail"),
    ("Outcome::Fault", "fault"),
];

/// The code of `source` before its tests, without its comment lines.
fn code(source: &str) -> String {
    let end = source.find("#[cfg(test)]").unwrap_or(source.len());
    let lines = source[..end].lines();
    let code: Vec<&str> = lines
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect();
    code.join("\n")
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

/// The names of the test functions of `source`.
fn tests(source: &str) -> BTreeSet<&str> {
    let mut names = BTreeSet::new();
    let mut lines = source.lines().map(str::trim);
    while let Some(line) = lines.next() {
        if line != "#[test]" {
            continue;
        }
        let function = lines.find(|line| !line.starts_with("#[") && !line.starts_with("//"));
        let name = function
            .and_then(|line| line.strip_prefix("fn "))
            .and_then(|line| line.split('(').next());
        names.extend(name);
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

/// The rules that the code of `source`, the file `file`, names in calls of
/// [`CALLS`], each with the results it gives them, into `rules`. A call
/// that is given another first argument than a string literal or the
/// `rule` that a helper passes on is a problem.
fn named_rules(
    file: &str,
    source: &str,
    rules: &mut BTreeMap<String, BTreeSet<String>>,
    problems: &mut Vec<String>,
) {
    let code = code(source);
    for (call, result) in CALLS {
        for (at, _) in code.match_indices(call) {
            let before = &code[..at];
            let joined = before
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
            if joined || before.ends_with("fn ") {
                continue;
            }
            let after = code[at + call.len()..].trim_start();
            let Some(literal) = after.strip_prefix('"') else {
                let passed = after.strip_prefix("rule");
                if !passed.is_some_and(|rest| rest.starts_with([',', ')'])) {
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

    // The requirement ids, from R01 in order in each prefix.
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
                "requirements: {prefix} has {found:?}, not R01 on in order"
            ));
        }
    }

    // The requirement tests: a known test of a known file for every row,
    // and a negative one for every requirement.
    let header = ["Requirement", "Test", "File", "Kind"];
    let rows = table(contract, "Requirement tests", &header).unwrap_or_else(|problem| {
        problems.push(problem);
        Vec::new()
    });
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
        match source(file) {
            None => problems.push(format!("contract: {file} is not a source of this test")),
            Some(text) if !tests(text).contains(test) => {
                problems.push(format!("contract: {file} has no test {test}"));
            }
            Some(_) => {}
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
    // requirements, their functions and their place on the formats page.
    let mut named = BTreeMap::new();
    for file in RULE_SOURCES {
        named_rules(file, source(file).unwrap_or(""), &mut named, &mut problems);
    }
    for rule in evidence_rules(source(EVIDENCE_SOURCE).unwrap_or("")) {
        named.entry(rule).or_default().insert("reason".to_string());
    }
    let header = ["Rule", "Group", "Result", "Function", "Formats"];
    let rows = table(contract, "Rule names", &header).unwrap_or_else(|problem| {
        problems.push(problem);
        Vec::new()
    });
    let mut listed = BTreeSet::new();
    for row in &rows {
        let [rule, group, result, functions, _] = row[..] else {
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
        let defined = |function: &&str| {
            let (module, name) = (function.split("::").next(), function.rsplit("::").next());
            let file = MODULES.iter().find(|(m, _)| Some(*m) == module);
            let text = file.and_then(|(_, file)| source(file)).map(code);
            name.zip(text)
                .is_some_and(|(name, text)| text.contains(&format!("fn {name}(")))
        };
        if functions.is_empty() || !functions.iter().all(defined) {
            problems.push(format!(
                "contract: a function of {rule} is not one of its module"
            ));
        }
        if !formats.contains(&format!("`{rule}`")) {
            problems.push(format!("formats: the rule {rule} is not on the page"));
        }
    }
    for rule in named.keys() {
        if !listed.contains(rule.as_str()) {
            problems.push(format!("contract: the rule names lack {rule}"));
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

#[test]
fn the_contract_lists_every_requirement_and_rule() {
    let none: Vec<String> = Vec::new();
    assert_eq!(problems(REQUIREMENTS, CONTRACT, FORMATS, &SOURCES), none);
    let header = ["Rule", "Group", "Result", "Function", "Formats"];
    let rules = table(CONTRACT, "Rule names", &header).map(|rows| rows.len());
    assert_eq!(rules, Ok(32));
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
    let source = |file: &str, from: &str, to: &str| {
        let text = SOURCES.iter().find(|(path, _)| *path == file).unwrap().1;
        assert!(text.contains(from), "{from}");
        let edited = text.replacen(from, to, 1);
        let pick = |&(path, text): &(&'static str, &'static str)| {
            (path, if path == file { edited.as_str() } else { text })
        };
        let sources: Vec<(&str, &str)> = SOURCES.iter().map(pick).collect();
        problems(REQUIREMENTS, CONTRACT, FORMATS, &sources)
    };
    // A heading that is not an id, and ids with a gap or out of order.
    let found = requirements("### VERD-R12", "### VERD-R1");
    assert_finds(&found, "VERD-R1 is not a requirement id");
    assert_finds(&requirements("### VERD-R12", "### VERD-R13"), "VERD has");
    assert_finds(&requirements("### PROV-R02", "### PROV-R03"), "PROV has");
    let found = requirements("### POWER-R06", "### power-R06");
    assert_finds(&found, "power-R06 is not a requirement id");
    // A requirement without a row or without a negative one, and a row of
    // another requirement, anchor, test, file or kind.
    let row = "| [POWER-R05](requirements.md#power-r05) \
        | `a_run_that_panics_switches_the_supply_off_and_leaves_no_output` \
        | `tools/kyto-bench/tests/run.rs` | negative |\n";
    let edit = |from: &str, to: &str| contract(row, &row.replace(from, to));
    assert_finds(&contract(row, ""), "POWER-R05 has no test");
    assert_finds(
        &edit("negative", "positive"),
        "POWER-R05 has no negative test",
    );
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
    let found = edit("tests/run.rs", "tests/manifest.rs");
    assert_finds(&found, "manifest.rs is not a source of this test");
    assert_finds(&edit("negative", "fuzz"), "fuzz is not a kind of test");
    assert_finds(
        &contract(row, &format!("{row}{row}")),
        "names a_run_that_panics",
    );
    // A rule of the code that the table lacks, or lists with another result,
    // group or function; a rule that the code or the formats page does not
    // name; and a rule listed twice.
    let rule = "| `device.error` | `POWER-R03` | fault | `power::device_errors` \
        | [checks](formats.md#checks) |\n";
    let edit = |from: &str, to: &str| contract(rule, &rule.replace(from, to));
    assert_finds(&contract(rule, ""), "the rule names lack device.error");
    assert_finds(
        &edit("| fault |", "| fail |"),
        "the code gives device.error",
    );
    assert_finds(&edit("POWER-R03", "POWER-R09"), "the group of device.error");
    let found = edit("device_errors", "device_faults");
    assert_finds(&found, "a function of device.error");
    let found = edit("power::device_errors", "verdict::device_errors");
    assert_finds(&found, "a function of device.error");
    let found = edit("device.error", "device.failure");
    assert_finds(&found, "the code does not name the rule device.failure");
    assert_finds(
        &found,
        "formats: the rule device.failure is not on the page",
    );
    let found = contract(rule, &format!("{rule}{rule}"));
    assert_finds(&found, "the rule device.error is listed twice");
    let formats = FORMATS.replace("`identity.disagree`", "`identity.differ`");
    let found = problems(REQUIREMENTS, CONTRACT, &formats, &SOURCES);
    assert_finds(
        &found,
        "formats: the rule identity.disagree is not on the page",
    );
    // A rule that the code names under another name or with another result,
    // a rule that is not a literal, a rule of evidence that is gone, and a
    // test that is not one.
    let verdict = "tools/kyto-bench-core/src/verdict.rs";
    let found = source(verdict, "\"identity.disagree\"", "\"identity.split\"");
    assert_finds(&found, "the rule names lack identity.split");
    assert_finds(&found, "the code does not name the rule identity.disagree");
    let payload = "tools/kyto-bench-core/src/payload.rs";
    let found = source(payload, "Outcome::Fault,\n", "Outcome::Fail,\n");
    assert_finds(&found, "the code gives payload.served_digest");
    let power = "tools/kyto-bench-core/src/power.rs";
    let found = source(power, "fault(\"power.order\"", "fault(order_rule");
    assert_finds(
        &found,
        "power.rs: fault( is given a rule that is no literal",
    );
    let receipt = "tools/kyto-bench-core/src/receipt.rs";
    let found = source(receipt, "\"evidence.simulated\",\n", "");
    assert_finds(&found, "the code does not name the rule evidence.simulated");
    let run = "tools/kyto-bench/tests/run.rs";
    let found = source(run, "#[test]\nfn a_run_that_panics", "fn a_run_that_panics");
    assert_finds(&found, "run.rs has no test a_run_that_panics");
}
