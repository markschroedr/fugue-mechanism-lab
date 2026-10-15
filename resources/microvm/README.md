# General coding microVM

A small persistent Linux coding sandbox for Apple Silicon, using vfkit. It uses Apple's hardware virtualization directly—no
Lima, Docker, host-folder mount, or second agent runtime. Outbound internet
access is enabled by default through vfkit NAT, so coding agents can fetch
documentation, clone repositories, and download dependencies normally.

## First use

```bash
brew install vfkit e2fsprogs zstd
./sandbox build
SANDBOX_CODEX_AUTH=/explicit/path/to/auth.json ./sandbox sync-openai-auth my-environment
./sandbox shell base
```

The base image has Bash, Git, Python, `uv`, Bun, and Pi installed. Pi includes
`pi-subagents` plus two read-only roles: Luna-high `explore` and Sol-medium
`review`. At most three run concurrently, and the parent agent always
implements. The base also includes Pi-native `review-triad` and `simplify`
skills under `~/.pi/agent/skills/`. Pi receives
a copied ChatGPT/Codex access token from the explicit `SANDBOX_CODEX_AUTH` source.
The guest receives no refresh token. Expired tokens require operator rearming;
Pi cannot rotate the host's credential. Credentials live in the selected private disk image.

`import-home` restores an additive, trusted `/home/coder` archive into a stopped
environment. It rejects absolute paths, parent traversal, and device entries.
Use a fresh base clone when moving a persistent environment between hosts.

```bash
./sandbox copy base my-environment
./sandbox import-home my-environment /path/to/coder-home.tar.gz
```

Refresh the package and policy in an existing environment with:

```bash
./sandbox setup-subagents fugue-run-1
```

Inside the VM, the primary working directory is:

```text
/home/coder/project
```

Exit the shell or run `sudo poweroff` to return to macOS.

## Make disposable copies

```bash
./sandbox copy base experiment-one
./sandbox shell experiment-one
./sandbox run experiment-one -- git status
./sandbox delete experiment-one
./sandbox list
```

Copies use APFS copy-on-write, so they start cheaply and diverge independently.
There is intentionally no host filesystem mount: the coding agent has normal
web access and can change its project and VM, but it cannot roam through the
Mac.

Import an existing private local branch without creating a remote or copying
host build artifacts:

```bash
./sandbox copy base fugue-run-1
./sandbox import-git fugue-run-1 "/path/to/local/repo" claude/run-1 /home/coder/project
./sandbox import-git fugue-run-1 "/path/to/related-repo" claude/run-1 /home/coder/repositories/related
./sandbox shell fugue-run-1
```

The import uses a Git bundle, verifies the exact commit, and removes the
temporary bundle remote. To add a guest-only environment value, pipe it over
standard input; it is stored as mode `0600` inside that environment's disk:

```bash
read -rs OPENROUTER_API_KEY
printf '%s\n' "$OPENROUTER_API_KEY" | ./sandbox set-env fugue-run-1 OPENROUTER_API_KEY
unset OPENROUTER_API_KEY
```

For an untrusted autonomous agent, remove guest sudo and restrict private-network
access before its first run:

```bash
./sandbox restrict-network my-environment 192.168.64.1 49325
```

This restriction is permanent for that environment. Recreate a clone from
`base` if it needs unrestricted coding-sandbox access again. Public internet
access remains available. The guest cannot reach the Mac, the LAN, link-local
services, or Tailscale addresses except for the declared endpoint.

## Run a recoverable Git job

Use `git-job` for a long coding-agent command. The sandbox commits all guest
changes and exports the result branch after the command returns. It imports the
exact exported commit into the host repository as an immutable branch named
`sandbox/<environment>/<guest-branch>/<commit>` and verifies that branch before
deleting the environment. The bundle remains as a second recovery copy.

```bash
./sandbox git-job experiment-one /home/coder/project main result/article \
  /tmp/result.bundle /path/to/host/repository -- agent-command
```

If the command, export, or host import fails, the environment disk remains
available. A failed command is checkpointed and imported when possible, but its
environment is retained for inspection. Recover current Git state without
rerunning the agent:

```bash
./sandbox export-git experiment-one /home/coder/project main result/article \
  /tmp/recovered.bundle /path/to/host/repository
```

Import a manually exported bundle into the matching host repository with
`git fetch`. Keep the environment until the host verifies the exported commit.

Move persistent home state between fresh environments without copying raw disks:

```bash
./sandbox export-home old-agent /tmp/old-agent-home.tar.gz
./sandbox import-home new-agent /tmp/old-agent-home.tar.gz
```

The export keeps repositories, sessions, identity, configuration, and notes. It
omits package and download caches that the preinstalled base can reproduce. The
host verifies the archive checksum before accepting it.

## Defaults

- 2 vCPUs
- 1 GiB RAM
- 6 GiB sparse disk per base image
- Alpine Linux 3.22 arm64
- vfkit NAT networking

Override resources for a launch with `SANDBOX_CPUS`, `SANDBOX_MEMORY_MIB`, and
`SANDBOX_DISK_GIB` (the disk setting applies only while creating `base`).

Generated assets and environment disks are in `assets/` and `state/`; neither
belongs in Git or a portable template archive.
