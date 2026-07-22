# agent workdirs

- each agent has unrestricted access to its own git worktree
- subagents inherit the worktree state from the parent
  - on spawn we create a commit that captures the state of the parent's
    worktree; `inspect` diffs against it, so that it doesn't get noisy when the
    parent makes concurrent edits

on linux we use fuse-overlayfs and bindfs to make them cheaper:

- agent worktrees are created with with no-checkout, and mounted on top of a
  shared commit snapshot
- to share gitignored stuff like compilation cache or .env files, we use a
  separate per-project lowerdir, where paths specified in the config are
  bind-mounted/hard-linked
