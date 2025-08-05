# repod

A CLI tool that recursively processes repositories, generates directory structures, and outputs repository contents in a format optimized for analysis.

## Features

- Clone and process Git repositories (HTTPS or SSH)
- Process local directories
- Generate directory tree structures 
- Filter files by language/repository type
- Exclude directories or file patterns
- Copy output to clipboard or save to file
- Detect and skip binary files
- Process large repositories efficiently with parallel processing
- Respects `.gitignore` files at all directory levels
- Automatically excludes hidden files and directories (starting with `.`)

## Installation

```bash
# Build from source
git clone https://github.com/yourusername/repod.git
cd repod
cargo build --release

# After building with cargo build --release
# Move to /usr/local/bin (requires sudo on most systems)
sudo cp target/release/repod /usr/local/bin/

# OR to user's bin directory (no sudo required)
mkdir -p ~/.local/bin
cp target/release/repod ~/.local/bin/

# Make sure ~/.local/bin is in your PATH
# Add this line to your .bashrc, .zshrc, or other shell config if needed:
# export PATH="$HOME/.local/bin:$PATH"

# The binary will be in target/release/repod
```

## Usage

```bash
# Basic usage (process current directory)
repod

# Clone and process a GitHub repository
repod https://github.com/username/repo

# Clone with SSH
repod git@github.com:username/repo.git

# Process and save output with a custom output directory
repod -o custom_output

# Specify repository types to filter
repod -t rust,python,javascript

# Exclude specific directories or patterns
repod -e node_modules,target,build

# Only include files matching specific patterns
repod --only "*.mdx,*.tsx"

# Copy output to clipboard instead of saving to file
repod --copy

# Clone to a specific location
repod https://github.com/username/repo --at /path/to/clone

# Open in Cursor IDE after cloning
repod git@github.com:username/repo.git --open-cursor
```

## Authentication

For private repositories:

```bash
# Using GitHub token
repod https://github.com/username/private-repo --github-token YOUR_TOKEN

# Or set the token as an environment variable
export GITHUB_TOKEN=your_token
repod https://github.com/username/private-repo

# Using SSH with a custom key
repod git@github.com:username/private-repo.git --ssh-key ~/.ssh/custom_key
```

## Options

```
Options:
  -o, --output-dir <OUTPUT_DIR>  Output directory path [default: output]
  -t, --repo-types <REPO_TYPES>  Repository types to filter files (e.g., rs, py, js, ts)
  -p, --github-token <GITHUB_TOKEN>  GitHub personal access token for private repositories
  -e, --exclude <EXCLUDE>        Additional folder or path patterns to exclude from processing
      --only <ONLY>              Only include files matching these patterns (e.g., *.mdx, *.tsx)
      --ssh-key <SSH_KEY>        SSH key path (defaults to ~/.ssh/id_rsa)
      --ssh-passphrase <SSH_PASSPHRASE>  SSH key passphrase (if not provided, will prompt if needed)
      --open-cursor              Open in Cursor after cloning
      --at <AT>                  Specific path to clone the repository to
      --copy                     Copy output to clipboard instead of saving to file
  -h, --help                     Print help
  -V, --version                  Print version
```

## Output Format

The output contains:
- A directory structure section with a tree view of the repository
- File contents with path information
- Files are processed in chunks to handle large repositories efficiently

## Examples

### Basic Repository Processing

```bash
repod https://github.com/username/repo -o analysis
```

### Filter by Language and Exclude Directories

```bash
repod -t rust,go -e tests,examples,target
```

### Process Current Directory and Copy to Clipboard

```bash
repod . --copy
```

### Process Only Specific File Types

```bash
# Only include MDX and TSX files
repod --only "*.mdx,*.tsx"

# Only include files in specific directories matching a pattern
repod --only "src/**/*.rs,tests/**/*.rs"
``` 