# Momulus

An orchestrator for LLM skills on pull requests. It watches PR comments, and on
a `/llm <skill> [args]` command it runs a deterministic pipeline with exactly
**one** LLM step (the [pi](https://www.npmjs.com/package/@earendil-works/pi-coding-agent)
coding agent in a container), then publishes the result back to the PR.

```
Trigger ──► Job ──► Queue(SQLite) ──► Pipeline ──► Publisher
                                         │
         workspace → skill resolve → Runner(pi in docker) → validate
```

No web UI, no message brokers, no Kubernetes: SQLite and a single process. No
GitHub Actions and no self-hosted runners either — the service polls the API
itself.

## Why "Momulus"

Momus was the Greek god of mockery, blame and nitpicking — the one deity whose
entire job was finding fault with everyone else's work.

When the gods showed off their creations, Momus reviewed them. Zeus had made a
bull, and Momus complained that the horns should have been placed below the
eyes, so the bull could see what it was goring. Athena had built a house, and he
objected that it had no wheels, so you couldn't move away from bad neighbours.
Aphrodite was, by his own admission, flawless — so he went after her sandals
instead, on the grounds that they squeaked when she walked.

That is exactly the register of a thorough code review: technically correct,
relentlessly specific, and faintly exhausting. The gods eventually threw him out
of Olympus for it. This one stays in your CI.

("Momulus" rather than "Momus" because the name was free, and because a small
diminutive suffix felt right for a bot that argues about a missing comma.)

## What it does

- **review skills** — the model finds issues, the bot publishes a single review
  with inline comments and ```suggestion``` blocks. Findings on lines outside
  the diff, or about files outside the skill's scope, are not discarded — they
  move into the summary.
- **patch skills** — the model edits files in the working copy, the bot commits
  under its own identity, pushes a branch and opens a PR whose base is the head
  branch of the original PR.
- Status is visible as reactions on the command comment: 👀 picked up,
  ✅ done, ❌ failed (plus a comment with a short reason and the job id).

Three skills ship with the project: `proofread` (proofreading Markdown/MDX blog
posts in any language), `review` (code review), and `translate` (post
translation, patch mode).

## Requirements

- Linux x86_64, 4–8 GB RAM. No public IP needed.
- Docker and docker-compose.
- A GitHub App (setup below) and an LLM provider key.

## Quick start

```bash
# 1. Build the pi runner image and the service image
just build-images

# 2. Validate the skills
just skills-validate

# 3. Run a skill locally, without GitHub — handy for iterating on prompts
export LLM_API_KEY=...            # provider key, e.g. an OpenAI one
cargo run -p momulus -- run \
    --repo-path ~/Projects/blog \
    --skill proofread \
    --files content/posts/dns/index.md \
    --out ./out
```

You get `./out/findings.json` (the findings) and `./out/pi.log` (the agent's
full output). For patch skills the edits stay in the working copy — inspect them
with `git diff`.

## Creating the GitHub App

1. **Settings → Developer settings → GitHub Apps → New GitHub App.**
2. Homepage URL can be anything; webhooks can stay off — the bot polls.
3. Repository permissions:

   | Permission | Level | Why |
   |---|---|---|
   | Contents | Read and write | clone the repository, push the patch branch |
   | Pull requests | Read and write | read PRs, publish reviews, open PRs |
   | Issues | Read and write | PR comments and reactions on them |
   | Metadata | Read-only | mandatory |

4. **Generate a private key** and download the `.pem`.
5. **Install App** on the repositories you want.
6. Note the **App ID** from the app page — it goes into `GITHUB_APP_ID`.

The service resolves the installation per repository on its own
(`GET /repos/{owner}/{repo}/installation`) and caches installation tokens, so
several installations work from one process.

## Configuration

`config.toml` holds everything except secrets (see `config.example.toml`):

```toml
poll_interval = "45s"          # how often we poll GitHub
concurrency = 1                # jobs running at the same time
allowed_users = ["zhurik"]     # who may issue commands
repos = []                     # repository allowlist; empty = all installations
data_dir = "/srv/momulus/data" # database, repo cache, worktrees, logs
skills_dir = "/srv/momulus/skills"
shutdown_timeout = "5m"

[llm]
default_provider = "openai"    # built into pi, so no base URL is needed
default_model = "gpt-5.1"
# api = "openai-completions"   # only for providers pi does not know about

[docker]
runner_image = "momulus-runner:latest"
cpu = 2.0
memory_mb = 2048
# user = "1000:1000"           # if the service uid differs from the image uid

[limits]
max_diff_bytes = 400000        # oversized PR → refused without calling the model
max_file_bytes = 200000
max_changed_files = 100
```

Secrets come from the environment only:

| Variable | Meaning |
|---|---|
| `GITHUB_APP_ID` | GitHub App id |
| `GITHUB_APP_PRIVATE_KEY_PATH` | path to the App's `.pem` key |
| `LLM_API_KEY` | LLM provider key |
| `LLM_BASE_URL` | provider API base — only for an OpenAI-compatible gateway (e.g. `https://openrouter.ai/api/v1`) |

### Providers pi does not know

pi ships with built-in providers (`openai`, `anthropic`, `google`, …). For those,
`default_provider`/`default_model` are enough and `api` stays commented out.

For an OpenAI-compatible gateway — OpenRouter, a self-hosted vLLM, a corporate
proxy, any regional model service — set `api` and point `LLM_BASE_URL` at the
endpoint:

```toml
[llm]
default_provider = "openrouter"
default_model = "<model id as the gateway names it>"
api = "openai-completions"
```

```bash
export LLM_BASE_URL=https://openrouter.ai/api/v1
export LLM_API_KEY=...
```

Momulus then generates a `models.json` inside the container with `baseUrl`, `api`
and the model list. The key itself never reaches that file: it is written as
`$OPENROUTER_API_KEY`, and pi reads the value from the container environment. The
variable name is derived from the provider name (`openrouter` →
`OPENROUTER_API_KEY`) or set explicitly via `llm.api_key_env`.

Validation is eager: if `api` is set but `LLM_BASE_URL` is not, the service
refuses to start and says exactly that.

## Deploying with docker-compose

```bash
export MOMULUS_ROOT=/srv/momulus
mkdir -p $MOMULUS_ROOT/{data,secrets}
cp config.example.toml $MOMULUS_ROOT/config.toml   # adjust paths and allowed_users
cp -r skills $MOMULUS_ROOT/skills
cp ~/github-app.private-key.pem $MOMULUS_ROOT/secrets/github-app.pem

cat > .env <<'ENV'
MOMULUS_ROOT=/srv/momulus
GITHUB_APP_ID=123456
LLM_API_KEY=...
# LLM_BASE_URL=https://openrouter.ai/api/v1   # only for a custom gateway
ENV

docker compose --profile build build   # the pi runner image
docker compose up -d
docker compose logs -f momulus
```

### Why host and container paths are identical

The service does not run pi itself — it asks the docker daemon to create a
container and mount the working copy into it. The daemon lives on the host and
only understands **host paths**. That is why `data_dir` and `skills_dir` are
absolute and mounted onto themselves (`/srv/momulus/data:/srv/momulus/data`).
Mount them anywhere else and the runner receives a path that does not exist on
the host.

### The docker.sock risk

Exposing `/var/run/docker.sock` to a container is effectively root on the host:
whoever can create containers can mount `/` and step outside. The service is
written so that the model's container receives no GitHub token and nothing
beyond the provider key, but the socket access itself stays powerful.

A calmer option is [docker-socket-proxy](https://github.com/Tecnativa/docker-socket-proxy),
which only forwards the endpoints you allow:

```yaml
services:
  docker-proxy:
    image: tecnativa/docker-socket-proxy:latest
    environment:
      CONTAINERS: 1      # create/start/wait/logs/kill/remove
      IMAGES: 1          # inspect the runner image
      POST: 1            # without it containers cannot be created
      # everything else (VOLUMES, NETWORKS, EXEC, SWARM, INFO...) stays off
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    restart: unless-stopped

  momulus:
    environment:
      DOCKER_HOST: tcp://docker-proxy:2375
    # and drop the /var/run/docker.sock mount
```

This is not full isolation (the right to create containers with bind mounts
remains), but it removes everything unnecessary and leaves an auditable gateway.

## How it works inside

1. **Trigger** polls `GET /repos/{o}/{r}/issues/comments?since=…` and
   `…/pulls/comments?since=…` for every repository of the installation, once per
   `poll_interval`. The cursor per repository and stream lives in SQLite;
   deduplication is by comment id. Only PR comments starting with `/llm` from
   `allowed_users` are considered. Everything else is ignored silently; an
   unparseable command gets a reply listing the available skills. Rate limits:
   `X-RateLimit-*` and `Retry-After` are honoured, with exponential backoff and
   jitter.
2. **Queue** — the `jobs` table in SQLite: `queued/running/done/failed`,
   attempt count, error, log path. On startup anything left in `running` goes
   back to the queue. Only transient errors are retried (network, 5xx, 429,
   timeout), at most two retries.
3. **Workspace** — a bare cache at `data/repos/<owner>/<repo>.git`, and one
   `git worktree` per job at the PR head commit in `data/work/<job_id>`. The
   directory is always removed, including on panic. The token reaches git
   through a credential helper reading an environment variable: it never appears
   in an on-disk remote URL or in logs.
4. **Runner** — one pi container per job: `/work` (read-only for review,
   read-write for patch), `/skills` (read-only), `/out` (read-write), CPU/RAM
   limits, `cap_drop: ALL`, `no-new-privileges`, and the timeout from the skill
   contract (on timeout the container is killed). The agent's full stdout/stderr
   is stored at `data/logs/<job_id>.log`.
5. **Validate** — strict deserialization of `/out/findings.json` (unknown fields
   rejected). Invalid JSON gets exactly one retry with the parse error added to
   the prompt, then the job fails. Findings outside the diff hunks or about
   other files move into the summary; extras are trimmed by `max_comments` with
   a counter.
6. **Publisher** — one `POST /repos/{o}/{r}/pulls/{n}/reviews` with
   `event=COMMENT` for review mode. For patch mode: commit, branch (a short SHA
   is appended on collision), push, and `POST /pulls`. Patch mode is not
   supported for PRs from forks — the bot says so in a comment.

## CLI

```bash
momulus serve [--dry-run]            # main mode; --dry-run publishes nothing
momulus run --repo-path <dir> --skill <name> [--files a.mdx,b.mdx] [--arg k=v]
momulus skills list                  # what skills exist
momulus skills validate              # check the contracts
```

Global flags: `--config`, `--skills-dir`, `--log-format text|json`,
`--log-level`.

`SIGHUP` reloads the skills directory on a running service (if a new contract is
broken, the previous registry stays in place). `SIGTERM`/`SIGINT` shut it down:
no new jobs are picked up, running ones finish within `shutdown_timeout`.

## Adding a skill

A directory `skills/<name>/` with two files.

`SKILL.md` — instructions for the model, in Agent Skills format:

```markdown
---
name: my-skill
description: What it does and when it applies — the model routes on this description
---

# My skill

Instructions: what to look for, what not to do, how to shape the result.
```

`skill.toml` — the contract for the orchestrator:

```toml
mode = "review"                              # review | patch
tools = ["read", "grep", "find", "ls", "write"]  # passed to pi as --tools
files = ["**/*.md", "**/*.mdx"]              # filter over changed PR files; empty = all
args = []                                    # e.g. ["lang"] for translate
timeout = "10m"
model = ""                                   # empty = model from the config
max_comments = 30                            # review only
# branch = "llm/{skill}-{pr}"                # patch only; arg names work here too

[vars]                                       # free-form parameters passed into the prompt
# naming = "{dir}/{stem}.{lang}{ext}"
```

Details worth knowing:

- `write` in `tools` is mandatory: without it the agent cannot write
  `/out/findings.json`. For review that is still safe — `/work` is mounted
  read-only, so the only writable place is `/out`.
- If no changed file in the PR matches `files`, the bot posts "nothing to do"
  and **does not** call the model.
- `momulus skills validate` catches unknown fields, empty `tools` in patch mode,
  broken globs, `max_comments` in patch mode, and unknown placeholders in
  `branch`.

## Adding a platform (GitLab, Forgejo)

The core (`core`, `skills`, `workspace`, `queue`, `pipeline`) knows nothing
about octocrab or bollard. To add a platform, write a crate implementing these
traits from `momulus-core`:

| Trait | What to implement |
|---|---|
| `Trigger` | where commands come from: API polling, cursors via `CursorStore`, parsing `/llm` with `Command::parse` |
| `Publisher` | `ack` (reactions), `post_review` (review with inline comments), `push_and_open_pr`, `comment` |
| `GitAccess` | the git token and the refspecs that fetch the PR head (on GitHub: `refs/pull/<n>/head`) |

Everything else — queue, working copies, prompts, validation, retries — is
reused as is. Wire the dependency graph up in
`crates/momulus/src/commands/serve.rs`.

Ready-made helpers for tests: `MemoryCursorStore`, `StaticSkillCatalog`,
`LocalGitAccess`, `StdoutPublisher`, `FakeRunner`.

## Security

- Commands run only for `allowed_users`, with an optional repository allowlist.
- PR content is untrusted input (prompt injection). The agent has **no** GitHub
  token, a minimal tool set, a read-only working copy in review mode, and the
  commit and push are done by the orchestrator, not by the model.
- Secrets are never logged: tokens and keys are masked in git and pi output
  (`Redactor`, with tests), and error comments carry no stack traces.
- Input size is bounded: an oversized diff or file is refused with a comment
  before the model is called.

## Development

```bash
just test               # fmt + clippy + cargo test
just test-integration   # tests that need Docker and a real pi
just build-images       # momulus-runner and momulus
```

Workspace layout:

```
crates/
  core/            Job, PrRef, Finding, Patch, traits, errors, config, redaction
  skills/          skill registry, skill.toml, contract validation
  workspace/       bare cache, git worktree, unified diff parser, line mapping
  queue/           SQLite, migrations, job lifecycle, worker
  pipeline/        job orchestration, prompt, validation, rendering of outputs
  runner-docker/   Runner via bollard
  github/          Trigger (polling) + Publisher on octocrab
  momulus/         binary: clap, config, dependency graph wiring
skills/            proofread, translate, review
docker/            service Dockerfile and Dockerfile.runner with pi
migrations/        sqlx migrations
```

Tests never touch the network: GitHub is replaced by `wiremock`, the LLM by
`FakeRunner`, and the remote by a local bare repository in `tempfile`. Tests that
need Docker and a real pi are marked `#[ignore]` and run separately; without
`LLM_API_KEY` they report that and pass.

Code, comments, log messages and everything the bot posts to a pull request are
in English. Findings themselves follow the language of the file under review:
the prompt tells the model to write each note in the language of the text it is
reviewing, so a Russian or German post gets reviewed in its own language.
