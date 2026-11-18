# AGENTS.md

- **Server setup**: to install, configure, update or check the GameLink
  agent on someone's game server or Pterodactyl node, follow
  [docs/agent-setup.md](docs/agent-setup.md) step by step.
- **Changing the code**: follow the ground rules in
  [CONTRIBUTING.md](CONTRIBUTING.md); toolchains are in
  [docs/building.md](docs/building.md).
- **Worktrees**: other agents may be working in the main checkout at the same
  time. Unless you were given a dedicated worktree for this task, create one.
  Base new work on the latest default branch, for example
  `git fetch origin && git worktree add ../gamelink-<topic> -b <topic> origin/master`.
  For existing work, use the branch or commit you were asked to work on. Never
  switch branches in a checkout you did not create. After your pull request
  merges, check that the worktree is clean and its branch tip is in the merged
  PR, then remove the worktree (`git worktree remove <path>`) and delete its
  local branch. Leave both in place if any local work remains.
