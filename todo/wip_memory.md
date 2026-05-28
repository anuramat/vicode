# memory system

memory eval: for each task, do best-of-n with different subsets of memories, eg
n=2: one agent with full set, one with empty set

memory attribution: ask the agent to cite memories it used to solve the task in
the end, validate with ablation, track memory use history

memories should be collapsible: if a memory is bigger than threshold, only
inject a short description  -- same mechanism as with skills; should be
greppable too

sleep: after each multiturn, ask user "does this look good?" and save, and then
during sleep: for each old thread that had a positive rating at some point,
extract a potential new memory entry, rerun (no tools) from the point where the
first attempt at the corresponding task started. if agent gets to the same
solution faster (ie original took more than one turn, or a single turn but it
took more tokens), accept the candidate; should be performed right after the
original thread was closed (because prompt caching); it would be basically free
or even save tokens if we only run it when the prompt cache is about to expire;
should use the same model/effort as the original thread

relate memories to file hashes: if hashes changed, memory needs
validation/update; if it passed validation, we add the new hashes to its set;
needs to be debounced

vibe memories: codebase state, user preferences, current user mindset (features
vs refactor vs bugfixes)

separate user supplied prompt for memories -- "focus on memories for improving
X"

as in honcho, treat subagent calls as user prompts -- both train main agent how
to prompt subagents, and train subagents how to execute

