# General coding microVM

A standalone coding sandbox using vfkit and Apple Virtualization.

## Required behavior

- `./sandbox build` creates a persistent `base` Linux environment using vfkit,
  not Lima or a container runtime.
- The base environment contains Git, Bash, Python, `uv`, Bun, the Pi coding
  agent, and the official Claude Agent SDK runtime, plus a persistent primary project directory at
  `/home/coder/project`.
- Pi includes `pi-subagents` and exactly two read-only roles: Luna-high
  `explore` and Sol-medium `review`. Depth is one; the parent agent alone
  implements; parallel groups contain at most three children. Packaged roles
  remain disabled.
- The template installs `review-triad` and `simplify` under Pi's supported
  user-skill path, `~/.pi/agent/skills/`; no prompt relies on an undiscovered
  `/home/coder/skills` path. `simplify` performs its review directly and never
  delegates; only `review-triad` fans out.
- `./sandbox setup-subagents NAME` installs or refreshes that package, skill,
  and policy in an already-created environment.
- `./sandbox copy base NAME` creates an independently writable, copy-on-write
  environment without modifying `base`.
- `./sandbox shell NAME` opens an interactive serial console as `coder` in the
  project directory. The VM can reach the internet through vfkit NAT but sees
  no host filesystem.
- `./sandbox run NAME -- COMMAND...` runs a command in the project directory and
  powers the VM off afterward.
- `./sandbox sync-openai-auth NAME` copies only the explicit source's Codex access token; it never copies refresh material.
- Keep secrets outside source, arguments, and logs; expired access tokens fail without refreshing host authentication.
- `./sandbox import-git NAME SOURCE_REPO BRANCH DESTINATION` transfers an exact
  branch as a Git bundle into `/home/coder/project` or one declared
  `/home/coder/repositories/ID` path. Repeated imports create independent
  checkouts without copying linked-worktree metadata, generated dependencies,
  or a temporary bundle remote.
- `./sandbox set-env NAME KEY` reads a value from standard input, stores it only
  inside that guest with mode `0600`, and exports it for guest shells and runs
  without printing the value.
- `./sandbox restrict-network NAME HOST PORT` permanently removes guest sudo.
  It allows public internet access but rejects private, link-local, and Tailscale
  destinations except for the one declared host endpoint. The host-supplied
  initramfs reapplies the policy before each guest command.
- Environment disk images and generated boot assets stay outside Git under
  `state/` and `assets/`.

The base image may contain an operator's Pi credential after explicit auth sync. Do not distribute `state/` or derived environment images.
