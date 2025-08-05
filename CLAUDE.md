# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`repod` is a Rust CLI tool that processes repositories and generates structured outputs optimized for analysis. It can clone Git repositories, process local directories, and output repository contents with directory trees.

## Development Commands

### Build
```bash
cargo build              # Debug build
cargo build --release    # Release build (optimized)
```

### Test
```bash
cargo test                           # Run all tests
cargo test test_name                 # Run specific test
cargo test -- --test-threads=1       # Run tests sequentially
cargo test -- --nocapture           # Show test output
```

### Lint and Format
```bash
cargo fmt                # Format code
cargo fmt -- --check     # Check formatting without changes
cargo clippy             # Run linter
cargo clippy -- -W clippy::pedantic  # Run with stricter lints
```

### Run
```bash
cargo run -- [args]                    # Debug mode
cargo run --release -- [args]          # Release mode
./target/release/repod [args]          # Run compiled binary
```

## Architecture

### Core Components

1. **main.rs** - Entry point and core processing logic
   - CLI argument parsing using `clap`
   - Repository cloning with `git2`
   - File processing with parallel execution using `rayon`
   - Binary file detection and text encoding handling
   - Progress tracking with `indicatif`

2. **tree.rs** - Directory tree generation
   - Builds hierarchical representation of repository structure
   - Handles exclusion patterns
   - Formats tree output with proper indentation

### Key Features

- **Parallel Processing**: Uses `rayon` for concurrent file processing
- **Memory Mapping**: Uses `memmap2` for efficient large file handling
- **Binary Detection**: Checks file headers and content to skip binary files
- **Progress Indication**: Shows progress bars during processing
- **Clipboard Support**: Can copy output directly to clipboard with `copypasta`

### Processing Flow

1. Parse CLI arguments
2. Clone repository (if URL provided) or use local path
3. Build directory tree structure
4. Process files in parallel chunks
5. Generate output with tree structure and file contents
6. Save to file or copy to clipboard

### File Processing

- Files larger than 1MB use memory mapping
- Binary files are detected and skipped
- Text encoding is handled with `encoding_rs`
- Token counting uses `tiktoken-rs` (OpenAI's tokenizer)

### Exclusion Patterns

Default exclusions include:
- **Automatic**: All files and directories starting with `.` (dotfiles/dotfolders)
- **Gitignore**: Respects patterns in `.gitignore` file if present
- **Build artifacts**: `node_modules`, `target`, `dist`, `build`, `out`, `bin`, `coverage`
- **Python**: `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.tox`, `venv`, `.venv`, `env`, `.eggs`
- **JavaScript**: `.next`, `.nuxt`, `.parcel-cache`, `.turbo`, `.vercel`, `.output`
- **Version control**: `.git`, `.svn`, `.hg`
- **IDE**: `.idea`, `.vs`, `.vscode`
- **Lock files**: `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `Cargo.lock`, `poetry.lock`, `Pipfile.lock`, `composer.lock`, `Gemfile.lock`, `go.sum`, `mix.lock`, `flake.lock`, `pubspec.lock`, `packages.lock.json`
- **Binary files**: Detected by content analysis
- Additional patterns can be specified with `-e` flag