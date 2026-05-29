# Kapitoshka

> ![kapitoshka-mascot](./assets/kapitoshka_coding_agent.svg)

A coding agent built in Rust. It connects to any OpenAI-compatible inference server, reasons about your codebase, and uses tools to read files, write files, list directories, and run shell commands.

## Requirements

- Rust 1.85+
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
| `read_file` | Read the contents of a file |
| `write_file` | Write content to a file (creates missing directories) |
| `list_dir` | List directory contents |
| `run_shell` | Run a shell command (e.g. `cargo test`, `grep -r foo src/`) |

All file paths are resolved relative to `--dir`.

## License

See [LICENSE](LICENSE) for details.

## Acknowledgments

- Built on [rig](https://github.com/0xPlaygrounds/rig).
