// Copyright (C) 2026 Ryan Daum <ryan.daum@gmail.com>
// SPDX-License-Identifier: GPL-3.0-only

//! Validate native query operands during unit replacement. A rejected candidate
//! is rolled back before the session can publish or execute its syntax policy.

use super::*;
use crate::syntax::{IndentRule, SyntaxPlan};

impl MicaHost {
    pub(super) async fn validate_syntax_policy(&mut self) -> Result<(), MicaHostError> {
        // Observe declarations only. This does not invoke an editor selector or
        // decide which mode/package should apply to a buffer.
        let source = r#"
let plans = []
let count = 0
for parser in roe/SyntaxParser(?mode, ?grammar, ?query)
  count < 64 || raise E_INVARG, "syntax parser limit exceeded"
  count = count + 1
  let rules = []
  let rule_count = 0
  for rule in roe/IndentationRule(parser[:mode], ?query, ?anchor, ?offset, ?precedence, ?enabled)
    if rule[:enabled]
      rule_count < 64 || raise E_INVARG, "syntax indentation rule limit exceeded"
      rule_count = rule_count + 1
      rules = [@rules, rule]
    end
  end
  plans = [@plans, {:grammar -> parser[:grammar], :query -> parser[:query], :rules -> rules}]
end
plans
"#;
        let invocation = self.administrator.evaluate(source.into()).await?;
        let mut events = std::mem::take(&mut self.deferred_events);
        let mut pump = self.event_pump.take().ok_or(MicaHostError::Closed)?;
        let outcome = pump
            .drive_invocation(&invocation, |event| {
                self.record_background_event(event, &mut events)
            })
            .await;
        self.event_pump = Some(pump);
        self.deferred_events = events;
        let value = match outcome {
            InvocationOutcome::Completed(value) => value,
            other => {
                return Err(MicaHostError::Policy(format!(
                    "syntax validation did not complete: {other:?}"
                )));
            }
        };
        validate_plans(&value).map_err(MicaHostError::Policy)
    }
}

fn validate_plans(value: &Value) -> Result<(), String> {
    let plans = value.list_len().ok_or("syntax plans must be a list")?;
    if plans > 64 {
        return Err("syntax parser limit exceeded".into());
    }
    for index in 0..plans {
        let plan = value.list_get(index).ok_or("missing syntax plan")?;
        let grammar = symbol(&plan, "grammar")?;
        let query = string(&plan, "query")?;
        let rules = map_value(&plan, "rules").ok_or("missing indentation rules")?;
        let count = rules.list_len().ok_or("indentation rules must be a list")?;
        if count > 64 {
            return Err("indentation rule limit exceeded".into());
        }
        let mut decoded = Vec::new();
        for index in 0..count {
            let rule = rules.list_get(index).ok_or("missing indentation rule")?;
            decoded.push(IndentRule {
                query: string(&rule, "query")?,
                anchor: symbol(&rule, "anchor")?,
                offset: integer(&rule, "offset")?,
                precedence: integer(&rule, "precedence")?,
            });
        }
        SyntaxPlan::compile(&grammar, &query, &decoded)?;
    }
    Ok(())
}

fn string(value: &Value, name: &str) -> Result<String, String> {
    map_value(value, name)
        .and_then(|value| value.with_str(str::to_owned))
        .ok_or_else(|| format!("syntax {name} must be a string"))
}

fn symbol(value: &Value, name: &str) -> Result<String, String> {
    map_value(value, name)
        .and_then(|value| value.as_symbol())
        .and_then(Symbol::name)
        .map(str::to_owned)
        .ok_or_else(|| format!("syntax {name} must be a symbol"))
}

fn integer(value: &Value, name: &str) -> Result<i64, String> {
    map_value(value, name)
        .and_then(|value| value.as_int())
        .ok_or_else(|| format!("syntax {name} must be an integer"))
}
