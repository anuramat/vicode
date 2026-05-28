# SSH backend aka remote agents

basic idea: when creating an agent, in addition to existing local logic, we can
specify backend=SSH -- the filesystem/bash tool logic gets executed on a remote
host; LLM API request still happens locally.

## impl

for each hostname in config, create a socket tunnel on app start, scp
vicode binary if missing, and launch in server mode; remote vicode server
executes toolcalls.

note:
- if we lose SSH access (e.g. machine is down), the files would be inaccessible.
  would need to show this in UI, and make the tab readonly, unless user force
  dupes it to local ignoring the workdir
- we'll need to scp agent workdirs between machines, and to sync git repos
- every way to create an agent should support specifying a backend
- we will need per-host sandbox config

## open questions

- maybe, reuse the assistant selection logic for backends?
- how to make UI convenient?
