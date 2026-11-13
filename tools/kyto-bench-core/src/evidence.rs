//! Whether a receipt can be evidence (`docs/bench/formats.md`, evidence):
//! only the receipt of a run on real devices, made by a tool of a known
//! commit whose working tree had no changes, is.

use crate::receipt::{EVIDENCE_RULES, Evidence, Reason, Tool};

/// The evidence of a run made by `tool` on a bench whose devices are
/// `simulated` or not: the rules of [`EVIDENCE_RULES`] that apply, in that
/// order, each with what it found, and eligible exactly when none applies.
pub fn eligibility(tool: &Tool, simulated: bool) -> Evidence {
    let found = [
        tool.commit
            .is_none()
            .then_some("the commit of the tool is unknown"),
        match tool.clean {
            Some(true) => None,
            Some(false) => Some("the working tree of the tool had changes"),
            None => Some("whether the working tree of the tool had changes is unknown"),
        },
        simulated.then_some("the run used simulated devices"),
    ];
    let reasons: Vec<Reason> = EVIDENCE_RULES
        .iter()
        .zip(found)
        .filter_map(|(rule, detail)| {
            let (rule, detail) = (rule.to_string(), detail?.to_string());
            Some(Reason { rule, detail })
        })
        .collect();
    let eligible = reasons.is_empty();
    Evidence { eligible, reasons }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::Receipt;
    use crate::receipt::tests::example;

    fn rules(evidence: &Evidence) -> Vec<&str> {
        evidence
            .reasons
            .iter()
            .map(|reason| reason.rule.as_str())
            .collect()
    }

    #[test]
    fn evidence_needs_a_known_commit_a_clean_tree_and_real_devices() {
        let commit = Some("0123456789abcdef0123456789abcdef01234567".to_string());
        let tool = |commit: Option<String>, clean| Tool { commit, clean };
        let real = eligibility(&tool(commit.clone(), Some(true)), false);
        assert!(real.eligible && real.reasons.is_empty());
        let simulated = eligibility(&tool(commit.clone(), Some(true)), true);
        assert!(!simulated.eligible);
        assert_eq!(rules(&simulated), ["evidence.simulated"]);
        assert_eq!(
            simulated.reasons[0].detail,
            "the run used simulated devices"
        );
        for clean in [Some(false), None] {
            let changed = eligibility(&tool(commit.clone(), clean), false);
            assert_eq!(rules(&changed), ["evidence.tree_changed"]);
            assert!(!changed.eligible);
        }
        let unknown = eligibility(&tool(None, Some(true)), false);
        assert!(!unknown.eligible);
        assert_eq!(rules(&unknown), ["evidence.commit_unknown"]);
        let every = eligibility(&tool(None, None), true);
        assert!(!every.eligible);
        assert_eq!(rules(&every), EVIDENCE_RULES);
    }

    #[test]
    fn a_receipt_records_the_evidence_of_its_tool_and_bench() {
        let mut receipt = example();
        assert_eq!(eligibility(&receipt.tool, true), receipt.evidence);
        receipt.tool.commit = None;
        receipt.evidence = eligibility(&receipt.tool, receipt.bench.simulated);
        assert!(Receipt::parse(receipt.render().as_bytes()).is_ok());
    }
}
