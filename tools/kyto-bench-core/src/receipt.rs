//! The parts of the record of a run that its checks give: the result of a
//! rule on a subject, and the refusal of a run before any power command.

use serde::{Deserialize, Serialize};

/// The result of a rule evaluated on one subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Pass,
    Fail,
    Fault,
}

/// A rule evaluated on one subject, such as a payload file or a marker,
/// with a detail when it did not pass.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub rule: String,
    pub subject: String,
    pub result: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Why a run was refused before any power command: the rule, the index of
/// the entry it concerns, if any, and what was found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    pub rule: String,
    pub entry: Option<usize>,
    pub detail: String,
}

impl Refusal {
    pub(crate) fn new(rule: &str, entry: usize, detail: String) -> Self {
        let (rule, entry) = (rule.to_string(), Some(entry));
        Self {
            rule,
            entry,
            detail,
        }
    }
}
