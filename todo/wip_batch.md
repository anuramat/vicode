# batch jobs

basic idea: user-defined adhoc multiagent workflows; for now, two flavors:
- user writes bash script, script outputs N prompts, each prompt sent to a new agent
- user prompts primary, primary creates N siblings

bash variant: user provides prompt generator -- bash script that outputs N pairs
(commit/branch, prompt), each spawns an agent;

prompt variant: user provides a description, agent spawns *primary agents* (and
then dies/summarizes/coordinates); should be implemented as a rework of subagent
system -- basically just subagent tool mode with user-facing (ie primary agent)
subagents; initial agent should be able to use logic from bash variant as a tool

batch job feature must include a job queue: at most n in-progress threads, and
threads start in the order they are given by the script/agent, so that if order
was "most important to least important", we would do important things first, and
could abort if we think it's enough
