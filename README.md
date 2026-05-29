# Kapitoshka

> ![kapitoshka-mascot](./assets/kapitoshka_coding_agent.svg)

A coding agent built in Rust. It connects to any OpenAI-compatible inference server, reasons about your codebase, and uses tools to read files, write files, list directories, and run shell commands.

## Requirements

- An OpenAI-compatible inference server (OpenAI, Ollama, vLLM, LM Studio, llama.cpp, etc.)

## Setup

```bash
export OPENAI_API_KEY=your-key          # required (use any string for local servers)
export OPENAI_BASE_URL=http://127.0.0.1:8080/v1  # omit to use OpenAI directly
```

## Usage

```bash
cargo run -- --task "add error handling to the database module" --dir /path/to/project
```

### Options

| Flag | Short | Default | Description |
| ------ | ------- | --------- | ------------- |
| `--task` | `-t` | *(required)* | The task for the agent to perform |
| `--dir` | `-d` | `.` | Working directory (root for all file operations) |
| `--model` | `-m` | `Qwen3-0.6B` | Model name to use |

### Tracing

Logs are written to stderr. Control verbosity with `RUST_LOG`:

```bash
RUST_LOG=debug cargo run -- --task "..."   # verbose: shows every tool call
RUST_LOG=info  cargo run -- --task "..."   # default: task start + tool names
RUST_LOG=error cargo run -- --task "..."   # quiet: errors only
```

## Tools

| Tool | Description |
| ------ | ------------- |
| `read_file` | Read a file, optionally limited to a line range (`start_line`, `end_line`) to avoid flooding the context window |
| `write_file` | Write content to a file (creates missing directories). Full overwrite — prefer `patch_file` for edits |
| `patch_file` | Replace an exact string in a file with new content. Errors if the match is ambiguous or missing |
| `list_dir` | List directory contents |
| `search_file` | Search for a literal string in a file and return matching lines with line numbers |
| `run_shell` | Run a shell command (e.g. `cargo test`, `grep -r foo src/`). Destructive commands are blocked |

All file paths are resolved relative to `--dir`.

### Shell safety

`run_shell` enforces a blocklist before executing any command. The following are blocked unconditionally:

| Category | Examples |
| --------- | -------- |
| Filesystem destruction | `rm -rf /`, `mkfs`, `dd if=`, `> /dev/sda` |
| Privilege escalation | `sudo`, `su -`, `pkexec` |
| Outbound network | `curl`, `wget`, `ssh`, `scp`, `rsync`, `nc` |
| Irreversible git | `git push` (any form) |
| Resource exhaustion | fork bombs |
| Shell escapes | `eval`, `exec` |

Safe commands such as `cargo test`, `git status`, `grep`, and `rm -rf target/` pass through unaffected.

## License

See [LICENSE](LICENSE) for details.

## Acknowledgments

- Built on [rig](https://github.com/0xPlaygrounds/rig).
