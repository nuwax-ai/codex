You are performing a CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task.

Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, or references needed to continue

Also carry forward, explicitly:
- Verified conclusions and the measurements that proved them. Keep the numbers; a conclusion without its evidence cannot be re-checked.
- Hypotheses and defects that were already investigated and ruled out, so the next LLM does not report them again as new findings.
- Files, commands, and environment details that were changed or are required to continue.
- Standing user constraints and preferences that must survive the handoff, even when they are unrelated to the current step.

Prefer concrete facts over narrative, and mark which items are verified versus assumed.
