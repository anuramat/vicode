# agent overlay overview

we use fuse-overlayfs and bindfs to get cheap per-agent workdirs:

- each agent has unrestricted access to its own workdir -- an overlayfs mount on
  top of a commit snapshot.
  - commit snapshot lowerdirs are shared between agents; agent only owns an
    upperdir, which is copied whenever we duplicate agents.
  - workdirs for primary agents are additionally registered as git worktrees
    - we create the agent worktrees with no-checkout, and then mount the overlay
      in the same directoy, moving .git file into upper -- no unnecessary writes
- since initially agents only get whatever is in the commit, to share gitignored
  stuff like compilation cache or .env files, we use a separate per-project
  lowerdir, where paths specified in the config are bind-mounted/hard-linked
  - for now, we recreate this lowerdir on each app launch
