# maybe

- acp *client*, so we can integrate other agents
  - only worth it if it's the only way to use certain coding agent subscriptions
- let tools take an optional `stage` param (signed int, 0 by default): tools in
  a single stage execute in parallel, stages execute sequentially
  - not sure if worth it -- races haven't been a problem; keep the idea in mind
    and implement if races become a problem
- let agent grep in the compacted/parent context?
  - or just let it ask the other agent (or its compacted version)
- btw command -- clone current history mid turn, prompt, show output as a notification or smth
- workflows with tail compaction -- i.e. agent starts a task at (1), ends at (2), and span (1-2) gets compacted
- better load balancing strategy than round robin for assistants
