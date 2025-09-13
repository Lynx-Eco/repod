use std::{
    fs::{ self, File },
    io::{ Write, BufReader, Read },
    path::Path,
    time::Instant,
    sync::Arc,
    path::PathBuf,
};
use glob::Pattern;
use anyhow::{ Context, Result };
use clap::Parser;
use git2::Repository;
use tempfile::TempDir;
use ignore::{WalkBuilder, DirEntry};
use tiktoken_rs::o200k_base;
use chrono::Local;
use parking_lot::Mutex;
use rayon::prelude::*;
use memmap2::Mmap;
use infer;
use dirs;
use copypasta::{ ClipboardContext, ClipboardProvider };
use indicatif::{ ProgressBar, ProgressStyle, MultiProgress, ParallelProgressIterator };
use std::process::Command;
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use crossterm::{terminal, event::{read, Event, KeyCode}};

mod tree;
use tree::DirectoryTree;

const LARGE_FILE_THRESHOLD: u64 = 1024 * 1024; // 1MB
const CHUNK_SIZE: usize = 100;
const BINARY_CHECK_SIZE: usize = 8192; // Increased binary check size
const TEXT_THRESHOLD: f32 = 0.3; // Maximum ratio of non-text bytes allowed

// Common text file extensions that we definitely want to include
const TEXT_EXTENSIONS: &[&str] = &[
    // Programming languages
    "rs",
    "py",
    "js",
    "ts",
    "java",
    "c",
    "cpp",
    "h",
    "hpp",
    "cs",
    "go",
    "rb",
    "php",
    "scala",
    "kt",
    "kts",
    "swift",
    "m",
    "mm",
    "r",
    "pl",
    "pm",
    "t",
    "sh",
    "bash",
    "zsh",
    "fish",
    // Web
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    "jsx",
    "tsx",
    "vue",
    "svelte",
    // Data/Config
    "json",
    "yaml",
    "yml",
    "toml",
    "xml",
    "csv",
    "ini",
    "conf",
    "config",
    "properties",
    // Documentation
    "md",
    "markdown",
    "rst",
    "txt",
    "asciidoc",
    "adoc",
    "tex",
    // Other
    "sql",
    "graphql",
    "proto",
    "cmake",
    "make",
    "dockerfile",
    "editorconfig",
    "gitignore",
];

// File patterns that should always be excluded
const EXCLUDED_PATTERNS: &[&str] = &[
    ".git/",
    "node_modules/",
    "target/",
    "build/",
    "dist/",
    "bin/",
    "__pycache__/",
    ".pytest_cache/",
    ".mypy_cache/",
    ".tox/",
    ".venv/",
    "venv/",
    "env/",
    ".env/",
    ".next/",
    ".nuxt/",
    ".cache/",
    ".parcel-cache/",
    ".turbo/",
    ".vercel/",
    ".output/",
    "coverage/",
    ".nyc_output/",
    ".eggs/",
    "*.egg-info/",
    ".svn/",
    ".hg/",
    ".DS_Store",
    ".idea/",
    ".vs/",
    ".vscode/",
    ".gradle/",
    "out/",
    "tmp/",
    ".tiktoken",
    ".bin",
    ".pack",
    ".idx",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "poetry.lock",
    "Pipfile.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
    "mix.lock",
    "flake.lock",
    "pubspec.lock",
    "packages.lock.json",
];

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Git repository URL, path to CSV file, or nothing to use current directory
    #[arg(index = 1)]
    input: Option<String>,

    /// Output directory path
    #[arg(short, long, default_value = "output")]
    output_dir: String,

    /// Repository types to filter files (e.g., rs, py, js, ts)
    /// Can specify multiple times for multiple types
    #[arg(short = 't', long, value_parser = parse_repo_type, value_delimiter = ',')]
    repo_types: Vec<RepoType>,

    /// GitHub personal access token for private repositories
    #[arg(short = 'p', long)]
    github_token: Option<String>,

    /// SSH key path (defaults to ~/.ssh/id_rsa)
    #[arg(long)]
    ssh_key: Option<String>,

    /// SSH key passphrase (if not provided, will prompt if needed)
    #[arg(long)]
    ssh_passphrase: Option<String>,

    /// Open in cursor after cloning
    #[arg(long)]
    open_cursor: bool,

    /// Specific path to clone the repository to
    #[arg(long)]
    at: Option<String>,

    /// Copy output to clipboard instead of saving to file (explicit)
    /// Default behavior is computed: copies for single-target runs unless --write or -o is set
    #[arg(long)]
    copy: bool,

    /// Write output to file instead of copying to clipboard (overrides default copy behavior)
    #[arg(long)]
    write: bool,

    /// Additional folder or path patterns to exclude from processing
    /// Can be specified multiple times or as a comma‑separated list
    #[arg(short = 'e', long = "exclude", value_delimiter = ',')]
    exclude: Vec<String>,

    /// Only include files matching these patterns (e.g., *.mdx, *.tsx)
    /// Can be specified multiple times or as a comma-separated list
    #[arg(long = "only", value_delimiter = ',')]
    only: Vec<String>,

    /// Stage and commit changes with an AI-generated message (single commit)
    /// Uses Gemini (models/gemini-2.5-flash) via GEMINI_API_KEY
    #[arg(long)]
    commit: bool,

    /// Analyze changes and propose multiple commits (current directory only)
    /// Uses Gemini (models/gemini-2.5-flash) via GEMINI_API_KEY
    #[arg(long = "multi-commit")]
    multi_commit: bool,
}

#[derive(Debug, Clone)]
enum RepoType {
    Rust,
    Python,
    JavaScript, // Now includes both JS and TS
    Go,
    Java,
}

fn parse_repo_type(s: &str) -> Result<RepoType, String> {
    match s.to_lowercase().as_str() {
        "rs" | "rust" => Ok(RepoType::Rust),
        "py" | "python" => Ok(RepoType::Python),
        "js" | "javascript" | "ts" | "typescript" => Ok(RepoType::JavaScript),
        "go" | "golang" => Ok(RepoType::Go),
        "java" => Ok(RepoType::Java),
        _ => Err(format!("Unknown repository type: {}", s)),
    }
}

fn get_repo_type_extensions(repo_type: &RepoType) -> &'static [&'static str] {
    match repo_type {
        RepoType::Rust => &["rs", "toml"],
        RepoType::Python =>
            &["py", "pyi", "pyx", "pxd", "requirements.txt", "setup.py", "pyproject.toml"],
        RepoType::JavaScript =>
            &["js", "jsx", "ts", "tsx", "json", "package.json", "tsconfig.json", "jsconfig.json"],
        RepoType::Go => &["go", "mod", "sum"],
        RepoType::Java => &["java", "gradle", "maven", "pom.xml", "build.gradle"],
    }
}

#[derive(Default)]
struct ProcessingStats {
    total_files: usize,
    total_tokens: usize,
    clone_time: f64,
    processing_time: f64,
    repo_count: usize,
    binary_files_skipped: usize,
}

struct FileContent {
    path: String,
    content: String,
    tokens: Vec<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Get URLs or use current directory
    let urls = if let Some(input) = &args.input {
        if input.ends_with(".csv") {
            // Check if file exists
            if !Path::new(input).exists() {
                anyhow::bail!("CSV file not found: {}", input);
            }
            read_urls_from_csv(input)?
        } else if input.starts_with("https://") || input.starts_with("git@") {
            vec![input.clone()]
        } else {
            anyhow::bail!(
                "Input must be either a CSV file or a git URL (https:// or git@). Got: {}",
                input
            );
        }
    } else {
        // Use current directory
        vec![".".to_string()]
    };

    // Check for GitHub token in environment if not provided as argument
    let args = if args.github_token.is_none() {
        let mut args = args;
        args.github_token = std::env::var("GITHUB_TOKEN").ok();
        args
    } else {
        args
    };

    let stats = Arc::new(Mutex::new(ProcessingStats::default()));
    let multi_progress = Arc::new(MultiProgress::new());

    // Determine if commit is allowed (only for current directory runs)
    let wants_commit = args.commit || args.multi_commit;
    let commit_allowed = wants_commit && urls.len() == 1 && urls[0] == ".";

    // Determine effective copy/write mode
    // Rules:
    // - --write forces writing to file
    // - --copy forces copying to clipboard
    // - Default (neither provided):
    //     * If multiple targets (CSV / multiple URLs): write to file to avoid clipboard races
    //     * Else if output_dir changed from default: write to file
    //     * Else: copy to clipboard
    let multiple_targets = urls.len() > 1;
    let copy_mode_global = if args.write {
        false
    } else if args.copy {
        true
    } else if multiple_targets || args.output_dir != "output" {
        false
    } else {
        true
    };

    // Only create output directory if we're writing to files and not in commit-only mode
    if !copy_mode_global && !commit_allowed {
        fs::create_dir_all(&args.output_dir)?;
    }

    if wants_commit && !commit_allowed {
        println!("--commit/--multi-commit only work on the current directory. Skipping commit.");
    }

    // Process repositories in parallel if there are multiple
    let do_parallel = urls.len() > 1;
    if do_parallel {
        urls
            .par_iter()
            .try_for_each(|url| {
                process_repository(
                    url,
                    &args.output_dir,
                    Arc::clone(&stats),
                    &args,
                    copy_mode_global,
                    commit_allowed && url == ".",
                    Arc::clone(&multi_progress)
                )
            })?;
    } else {
        process_repository(
            &urls[0],
            &args.output_dir,
            Arc::clone(&stats),
            &args,
            copy_mode_global,
            commit_allowed,
            Arc::clone(&multi_progress)
        )?;
    }

    let final_stats = stats.lock();
    if !commit_allowed {
        print_stats(&final_stats);
    }
    Ok(())
}

fn read_urls_from_csv(path: &str) -> Result<Vec<String>> {
    let mut urls = Vec::new();
    let mut reader = csv::Reader::from_path(path)?;
    for result in reader.records() {
        let record = result?;
        if let Some(url) = record.get(0) {
            urls.push(url.to_string());
        }
    }
    Ok(urls)
}

fn read_file_content(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;

    if metadata.len() > LARGE_FILE_THRESHOLD {
        // Log large file processing
        println!(
            "Processing large file ({:.2} MB): {}",
            (metadata.len() as f64) / 1024.0 / 1024.0,
            path.display()
        );
        // Use memory mapping for large files
        let mmap = unsafe { Mmap::map(&file)? };
        Ok(String::from_utf8_lossy(&mmap).into_owned())
    } else {
        // Use regular reading for small files
        // Read raw bytes first to handle potential non-UTF8 sequences
        let mut buffer = Vec::with_capacity(metadata.len() as usize);
        BufReader::new(file).read_to_end(&mut buffer)?;
        // Convert to string lossily, replacing invalid sequences
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }
}

fn process_files_batch(files: &[FileContent], output: &mut dyn Write) -> Result<()> {
    for file in files {
        // Write file info and content
        writeln!(output, "<file_info>")?;
        writeln!(output, "path: {}", file.path)?;
        writeln!(output, "name: {}", Path::new(&file.path).file_name().unwrap().to_string_lossy())?;
        writeln!(output, "</file_info>")?;
        writeln!(output, "{}\n", file.content)?;
    }
    Ok(())
}

fn handle_auth_error(url: &str, error: &git2::Error) -> anyhow::Error {
    let is_auth_error =
        error.code() == git2::ErrorCode::Auth ||
        error.message().contains("authentication") ||
        error.message().contains("authorization");

    if is_auth_error {
        let mut msg = String::from("\nAuthentication failed. To fix this:\n");

        if url.starts_with("https://") {
            msg.push_str(
                "For HTTPS repositories:\n\
                1. Set your GitHub token using one of these methods:\n\
                   - Run with --github-token YOUR_TOKEN\n\
                   - Set the GITHUB_TOKEN environment variable\n\
                2. Ensure your token has the 'repo' scope enabled\n"
            );
        } else if url.starts_with("git@") {
            msg.push_str(
                "For SSH repositories:\n\
                1. Ensure your SSH key is set up correctly:\n\
                   - Default location: ~/.ssh/id_rsa\n\
                   - Or specify with --ssh-key /path/to/key\n\
                2. Verify your SSH key is added to GitHub\n\
                3. Test SSH access: ssh -T git@github.com\n"
            );
        } else {
            msg.push_str(
                "Ensure you're using either:\n\
                - HTTPS URL (https://github.com/org/repo)\n\
                - SSH URL (git@github.com:org/repo)\n"
            );
        }

        anyhow::anyhow!(msg)
    } else {
        anyhow::anyhow!("Git error: {}", error)
    }
}

fn prompt_passphrase(pb: &ProgressBar) -> Result<String> {
    // Pause the spinner while waiting for input
    pb.set_message("Waiting for SSH key passphrase...");
    pb.disable_steady_tick();

    let passphrase = rpassword::prompt_password("Enter SSH key passphrase: ")?;

    // Resume the spinner
    pb.enable_steady_tick(std::time::Duration::from_millis(100));

    Ok(passphrase)
}

fn clone_repository(
    url: &str,
    path: &Path,
    args: &Args,
    multi_progress: &MultiProgress
) -> Result<Repository> {
    let mut callbacks = git2::RemoteCallbacks::new();
    let mut fetch_options = git2::FetchOptions::new();
    let mut builder = git2::build::RepoBuilder::new();

    // Create progress bar for cloning
    let clone_pb = multi_progress.add(ProgressBar::new_spinner());
    clone_pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg} [{elapsed_precise}]")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
    );
    clone_pb.enable_steady_tick(std::time::Duration::from_millis(100));

    let result = if url.starts_with("https://") {
        clone_pb.set_message(format!("Connecting to: {}", url));
        // Try without token first for public repos
        let result = builder.clone(url, path);
        if let Err(e) = result {
            if e.code() == git2::ErrorCode::Auth {
                clone_pb.set_message("Repository requires authentication, trying with token...");
                // If auth failed, try with token
                if let Some(token) = &args.github_token {
                    callbacks.credentials(|_url, _username_from_url, _allowed_types| {
                        git2::Cred::userpass_plaintext(token, "x-oauth-basic")
                    });
                    fetch_options.remote_callbacks(callbacks);
                    builder.fetch_options(fetch_options);
                    builder.clone(url, path).map_err(|e| handle_auth_error(url, &e))
                } else {
                    Err(
                        anyhow::anyhow!(
                            "Repository requires authentication.\n\
                        Please provide a GitHub token using --github-token or set the GITHUB_TOKEN environment variable."
                        )
                    )
                }
            } else {
                Err(handle_auth_error(url, &e))
            }
        } else {
            Ok(result.unwrap())
        }
    } else if url.starts_with("git@") {
        clone_pb.set_message(format!("Setting up SSH connection to: {}", url));

        let ssh_key_path = args.ssh_key
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
                PathBuf::from(home).join(".ssh/id_rsa")
            });

        if !ssh_key_path.exists() {
            clone_pb.finish_with_message("✗ SSH key not found");
            return Err(
                anyhow::anyhow!(
                    "SSH key not found at {}.\n\
                Please ensure your SSH key exists or specify a different path with --ssh-key",
                    ssh_key_path.display()
                )
            );
        }

        // First try without passphrase
        clone_pb.set_message(format!("Attempting SSH connection to: {}", url));
        let passphrase = args.ssh_passphrase.clone();
        callbacks.credentials(move |_url, _username_from_url, _allowed_types| {
            git2::Cred::ssh_key(
                _username_from_url.unwrap_or("git"),
                None,
                &ssh_key_path,
                passphrase.as_deref()
            )
        });
        fetch_options.remote_callbacks(callbacks);
        builder.fetch_options(fetch_options);

        let clone_result = builder.clone(url, path);

        if let Err(e) = &clone_result {
            if
                e.class() == git2::ErrorClass::Ssh &&
                e.message().contains("Unable to extract public key") &&
                args.ssh_passphrase.is_none()
            {
                // Try again with passphrase
                let passphrase = prompt_passphrase(&clone_pb)?;

                clone_pb.set_message(format!("Retrying SSH connection to: {}", url));
                let mut callbacks = git2::RemoteCallbacks::new();
                let ssh_key_path = args.ssh_key
                    .as_ref()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| {
                        let home = std::env::var("HOME").unwrap_or_else(|_| "~".to_string());
                        PathBuf::from(home).join(".ssh/id_rsa")
                    });

                callbacks.credentials(move |_url, _username_from_url, _allowed_types| {
                    git2::Cred::ssh_key(
                        _username_from_url.unwrap_or("git"),
                        None,
                        &ssh_key_path,
                        Some(&passphrase)
                    )
                });

                let mut fetch_options = git2::FetchOptions::new();
                fetch_options.remote_callbacks(callbacks);
                builder.fetch_options(fetch_options);

                builder.clone(url, path).map_err(|e| handle_auth_error(url, &e))
            } else {
                clone_result.map_err(|e| handle_auth_error(url, &e))
            }
        } else {
            clone_result.map_err(|e| handle_auth_error(url, &e))
        }
    } else {
        clone_pb.finish_with_message("✗ Invalid URL format");
        Err(
            anyhow::anyhow!(
                "Invalid repository URL format: {}\n\
            URL must start with 'https://' or 'git@'",
                url
            )
        )
    };

    // Update progress bar based on result
    match &result {
        Ok(_) => {
            if url.starts_with("git@") {
                clone_pb.finish_with_message(
                    format!(
                        "✓ SSH connection established and repository cloned in {:.1}s",
                        clone_pb.elapsed().as_secs_f64()
                    )
                );
            } else {
                clone_pb.finish_with_message(
                    format!("✓ Repository cloned in {:.1}s", clone_pb.elapsed().as_secs_f64())
                );
            }
        }
        Err(_) => {
            clone_pb.finish_with_message("✗ Failed to clone repository");
        }
    }

    result
}

fn process_repository(
    url: &str,
    output_dir: &str,
    stats: Arc<Mutex<ProcessingStats>>,
    args: &Args,
    copy_mode: bool,
    allow_commit: bool,
    multi_progress: Arc<MultiProgress>
) -> Result<()> {
    let clone_start = Instant::now();

    // Determine the repository directory
    let repo_dir = if url == "." {
        // Use current directory
        std::env::current_dir()?
    } else if let Some(path) = &args.at {
        PathBuf::from(path)
    } else if args.open_cursor {
        // Use cache directory for cursor mode if no specific path provided
        let cache_dir = dirs
            ::cache_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine cache directory"))?
            .join("repod");
        fs::create_dir_all(&cache_dir)?;
        cache_dir.join(extract_repo_name(url))
    } else {
        TempDir::new()?.into_path()
    };

    // Only clone if it's a remote repository
    if url != "." {
        // If directory exists and is not empty, remove it first
        if repo_dir.exists() {
            if repo_dir.read_dir()?.next().is_some() {
                println!("Directory exists and is not empty, removing: {}", repo_dir.display());
                fs::remove_dir_all(&repo_dir)?;
            }
        }

        let _repo = clone_repository(url, &repo_dir, args, &multi_progress).with_context(||
            format!("Failed to access repository: {}", url)
        )?;

        {
            let mut stats_guard = stats.lock();
            stats_guard.repo_count += 1;
            stats_guard.clone_time += clone_start.elapsed().as_secs_f64();
        }
    }

    // If commit-only mode is enabled, skip scanning/output and just run commit flow
    if allow_commit {
        if args.multi_commit {
            commit_with_ai_multi(&repo_dir, &multi_progress)?;
        } else if args.commit {
            commit_with_ai_single(&repo_dir, &multi_progress)?;
        }
        return Ok(());
    }

    let process_start = Instant::now();

    // Create tokenizer once
    let tokenizer = Arc::new(o200k_base().unwrap());

    // First, check for README file in root
    let scan_pb = multi_progress.add(ProgressBar::new_spinner());
    scan_pb.set_style(ProgressStyle::default_spinner().template("{spinner:.blue} {msg}").unwrap());
    scan_pb.enable_steady_tick(std::time::Duration::from_millis(100));
    scan_pb.set_message("Scanning repository structure...");

    let mut readme_content: Option<FileContent> = None;
    for readme_name in ["README.md", "README.txt", "README", "Readme.md", "readme.md"] {
        let readme_path = repo_dir.join(readme_name);
        if readme_path.exists() && readme_path.is_file() {
            // Check if README matches the only patterns
            if !args.only.is_empty() {
                let matches_pattern = args.only.iter().any(|pattern| {
                    if let Ok(glob_pattern) = Pattern::new(pattern) {
                        glob_pattern.matches(readme_name)
                    } else {
                        false
                    }
                });
                
                if !matches_pattern {
                    continue; // Skip this README if it doesn't match
                }
            }
            
            if let Ok(content) = read_file_content(&readme_path) {
                let tokens = tokenizer.encode_with_special_tokens(&content);
                readme_content = Some(FileContent {
                    path: readme_name.to_string(),
                    content,
                    tokens: tokens
                        .iter()
                        .map(|t| t.to_string())
                        .collect(),
                });
                break;
            }
        }
    }

    // Build combined list of excluded patterns (built‑in + user‑supplied)
    let excluded_patterns: Vec<&str> = EXCLUDED_PATTERNS.iter()
        .copied()
        .chain(args.exclude.iter().map(|s| s.as_str()))
        .collect();

    // Build the walker with ignore support
    let mut walker_builder = WalkBuilder::new(&repo_dir);
    
    // Configure the walker
    // For cloned repos, we disable git-specific ignores to ensure consistent behavior
    // regardless of how the repo was obtained (cloned vs downloaded)
    let is_cloned_repo = url != ".";
    
    
    walker_builder
        .hidden(false) // We'll handle hidden files with our own logic
        .git_ignore(true) // Always respect .gitignore files in the repo
        .git_global(!is_cloned_repo) // Only respect global gitignore for local repos
        .git_exclude(!is_cloned_repo) // Only respect .git/info/exclude for local repos
        .ignore(true) // Respect .ignore files
        .parents(!is_cloned_repo); // Only respect parent ignore files for local repos
    
    // Add custom ignore patterns
    for pattern in &excluded_patterns {
        walker_builder.add_custom_ignore_filename(format!(".{}", pattern));
    }

    // Count total files first for progress bar
    let total_files: usize = walker_builder.build()
        .filter_map(Result::ok)
        .filter(|entry| {
            let path = entry.path();
            let path_str = path.to_string_lossy();
            
            // Check our built-in exclusions
            let is_excluded = excluded_patterns.iter().any(|pattern| path_str.contains(pattern));
            
            // Check if it's a hidden file/folder (starts with .)
            // Only check path components RELATIVE to the repo_dir to avoid issues with temp directories
            let is_hidden = if let Ok(relative_path) = path.strip_prefix(&repo_dir) {
                relative_path.components().any(|component| {
                    if let std::path::Component::Normal(name) = component {
                        name.to_string_lossy().starts_with('.')
                    } else {
                        false
                    }
                })
            } else {
                // If we can't get relative path, check the full path (fallback)
                path.file_name()
                    .map(|name| name.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
            };
            
            let is_file = entry.file_type().map(|ft| ft.is_file()).unwrap_or(false);
            
            is_file && !is_excluded && !is_hidden
        })
        .count();

    scan_pb.finish_with_message(format!("Found {} files", total_files));

    // Process files progress bar
    let process_pb = multi_progress.add(ProgressBar::new(total_files as u64));
    process_pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} files ({eta})")
            .unwrap()
            .progress_chars("#>-")
    );
    process_pb.enable_steady_tick(std::time::Duration::from_millis(100));

    // Collect and process other files in parallel
    let files: Vec<_> = walker_builder.build()
        .filter_map(Result::ok)
        .filter(|entry| {
            let path = entry.path();
            let path_str = path.to_string_lossy();
            
            // Check our built-in exclusions
            let is_excluded = excluded_patterns.iter().any(|pattern| path_str.contains(pattern));
            
            // Check if it's a hidden file/folder (starts with .)
            // Only check path components RELATIVE to the repo_dir to avoid issues with temp directories
            let is_hidden = if let Ok(relative_path) = path.strip_prefix(&repo_dir) {
                relative_path.components().any(|component| {
                    if let std::path::Component::Normal(name) = component {
                        name.to_string_lossy().starts_with('.')
                    } else {
                        false
                    }
                })
            } else {
                // If we can't get relative path, check the full path (fallback)
                path.file_name()
                    .map(|name| name.to_string_lossy().starts_with('.'))
                    .unwrap_or(false)
            };
            
            entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) && !is_excluded && !is_hidden
        })
        .par_bridge()
        .progress_with(process_pb.clone())
        .filter_map(|entry: DirEntry| {
            let path = entry.path();
            // Skip if this is the README we already processed
            if let Some(ref readme) = readme_content {
                if path.file_name().and_then(|n| n.to_str()) == Some(&readme.path) {
                    return None;
                }
            }

            let should_process = should_process_file(path, if args.repo_types.is_empty() {
                None
            } else {
                Some(&args.repo_types)
            }, &args.only);
            let is_binary = matches!(is_binary_file(path), Ok(true));

            if !should_process || is_binary {
                if is_binary {
                    // Increment binary skipped counter if is_binary is true
                    stats.lock().binary_files_skipped += 1;
                }
                return None;
            }

            read_file_content(path)
                .ok()
                .map(|content| {
                    let relative_path = path.strip_prefix(&repo_dir).unwrap().display().to_string();
                    let tokens = tokenizer.encode_with_special_tokens(&content);
                    FileContent {
                        path: relative_path,
                        content,
                        tokens: tokens
                            .iter()
                            .map(|t| t.to_string())
                            .collect(),
                    }
                })
        })
        .collect();

    process_pb.finish_with_message(format!("Processed {} files", files.len()));

    // Update stats
    {
        let mut stats_guard = stats.lock();
        stats_guard.total_files += files.len() + (readme_content.is_some() as usize);
        stats_guard.total_tokens += files
            .iter()
            .map(|f| f.tokens.len())
            .sum::<usize>();
        if let Some(ref readme) = readme_content {
            stats_guard.total_tokens += readme.tokens.len();
        }
        stats_guard.processing_time += process_start.elapsed().as_secs_f64();
    }

    // Write progress
    let write_pb = multi_progress.add(ProgressBar::new_spinner());
    write_pb.set_style(
        ProgressStyle::default_spinner().template("{spinner:.green} {msg}").unwrap()
    );
    write_pb.enable_steady_tick(std::time::Duration::from_millis(100));
    write_pb.set_message("Writing output");

    // Create output content
    let mut output_buffer = Vec::new();

    // First, write the directory tree
    writeln!(&mut output_buffer, "<directory_structure>")?;
    let tree = DirectoryTree::build(&repo_dir, &excluded_patterns, &args.only)?;
    writeln!(&mut output_buffer, "{}", tree.format())?;
    writeln!(&mut output_buffer, "</directory_structure>\n")?;

    // Write README first if it exists
    if let Some(readme) = readme_content {
        process_files_batch(&[readme], &mut output_buffer)?;
    }

    // Write remaining files in chunks
    for chunk in files.chunks(CHUNK_SIZE) {
        process_files_batch(chunk, &mut output_buffer)?;
    }

    // Handle output based on mode
    if copy_mode {
        // Copy to clipboard
        let content = String::from_utf8(output_buffer)?;
        let mut ctx = ClipboardContext::new().map_err(|e|
            anyhow::anyhow!("Failed to access clipboard: {}", e)
        )?;
        ctx
            .set_contents(content)
            .map_err(|e| anyhow::anyhow!("Failed to copy to clipboard: {}", e))?;
        println!("Content copied to clipboard");
    } else {
        // Write to file
        let output_file_name = if args.open_cursor {
            // In cursor mode, write to the repo root
            let timestamp = Local::now().format("%Y%m%d_%H%M%S");
            repo_dir.join(format!("screenpipe_{}.txt", timestamp))
        } else {
            let timestamp = Local::now().format("%Y%m%d_%H%M%S");
            let repo_name = if url == "." {
                repo_dir.file_name().unwrap().to_string_lossy().to_string()
            } else {
                extract_repo_name(url)
            };
            PathBuf::from(format!("{}/{}_{}.txt", output_dir, repo_name, timestamp))
        };
        let mut file = File::create(&output_file_name)?;
        file.write_all(&output_buffer)?;
    }

    write_pb.finish_with_message("Finished writing output");

    // Make sure all progress bars are properly cleaned up
    drop(scan_pb);
    drop(process_pb);
    drop(write_pb);
    multi_progress.clear()?;

    // If cursor mode is enabled, run the cursor command
    if args.open_cursor {
        let cursor_cmd = format!("cursor {}", repo_dir.display());
        if let Err(e) = std::process::Command::new("sh").arg("-c").arg(&cursor_cmd).spawn() {
            println!("Failed to open Cursor: {}", e);
        }
    }

    Ok(())
}

// -------------------- Commit support --------------------

fn commit_with_ai_message(repo_dir: &Path) -> Result<()> {
    // Ensure we're in a git repository
    if !repo_dir.join(".git").exists() {
        anyhow::bail!("Not a git repository: {}", repo_dir.display());
    }

    // Detect any changes (staged or unstaged)
    let status_porcelain = run_in_repo(repo_dir, &["git", "status", "--porcelain"])?;
    if status_porcelain.trim().is_empty() {
        println!("No changes detected. Nothing to commit.");
        return Ok(());
    }

    // Build prompt from diff vs HEAD (includes both staged and unstaged)
    let name_status = run_in_repo(repo_dir, &["git", "diff", "--name-status", "HEAD"])?;
    let shortstat = run_in_repo(repo_dir, &["git", "diff", "--shortstat", "HEAD"])?;
    // Keep the diff small to avoid huge payloads; include a bit of context
    let diff_sample = run_in_repo(repo_dir, &["git", "diff", "-U3", "HEAD"])?;
    let diff_sample = truncate(&diff_sample, 10_000);

    let prompt = build_commit_prompt_multiline(&name_status, &shortstat, &diff_sample);
    let msg = match generate_commit_message_via_gemini(&prompt) {
        Ok(m) => m,
        Err(_) => fallback_commit_message_multiline(&name_status, &shortstat),
    };

    // Show and confirm
    println!("Proposed commit message:\n\n{}", msg);
    if !prompt_yes_no("Commit with this message? [y/N] ")? {
        println!("Commit canceled by user.");
        return Ok(());
    }

    // Stage all changes and commit
    run_in_repo(repo_dir, &["git", "add", "-A"])?.to_string();
    let commit_res = run_in_repo(repo_dir, &["git", "commit", "-m", &msg]);
    match commit_res {
        Ok(_) => { println!("Committed with AI message: {}", msg); Ok(()) }
        Err(e) => Err(e),
    }
}

fn commit_with_ai_choice(repo_dir: &Path, multi_progress: &MultiProgress) -> Result<()> {
    // Ensure repo and changes
    if !repo_dir.join(".git").exists() {
        anyhow::bail!("Not a git repository: {}", repo_dir.display());
    }
    let status_porcelain = run_in_repo(repo_dir, &["git", "status", "--porcelain"])?;
    if status_porcelain.trim().is_empty() {
        anyhow::bail!("no changes to commit");
    }

    // Produce single-commit proposal (multi-line)
    let name_status = run_in_repo(repo_dir, &["git", "diff", "--name-status", "HEAD"])?;
    let shortstat = run_in_repo(repo_dir, &["git", "diff", "--shortstat", "HEAD"])?;
    let diff_sample = truncate(&run_in_repo(repo_dir, &["git", "diff", "-U3", "HEAD"])? , 20_000);
    let pb_single = multi_progress.add(ProgressBar::new_spinner());
    pb_single.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg} [{elapsed_precise}]").unwrap());
    pb_single.enable_steady_tick(std::time::Duration::from_millis(100));
    pb_single.set_message("Generating single-commit proposal...");
    let single_prompt = build_commit_prompt_multiline(&name_status, &shortstat, &diff_sample);
    let single_msg = match generate_commit_message_via_gemini(&single_prompt) {
        Ok(m) => m,
        Err(_) => fallback_commit_message_multiline(&name_status, &shortstat),
    };
    pb_single.finish_with_message("Single-commit proposal ready");

    // Try to produce multi-commit plan
    let pb_multi = multi_progress.add(ProgressBar::new_spinner());
    pb_multi.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg} [{elapsed_precise}]").unwrap());
    pb_multi.enable_steady_tick(std::time::Duration::from_millis(100));
    pb_multi.set_message("Analyzing multi-commit plan...");
    let multi_plan = plan_multi_commits(repo_dir, multi_progress).ok();
    pb_multi.finish_with_message("Multi-commit analysis complete");
    let has_sensible_multi = multi_plan
        .as_ref()
        .map(|(commits, _)| commits.len() >= 2)
        .unwrap_or(false);

    // Show options
    println!("Option A: Single commit message:\n\n{}\n", single_msg);
    if has_sensible_multi {
        let (commits, leftovers) = multi_plan.as_ref().unwrap();
        println!("Option B: Multi-commit plan ({} commits):\n", commits.len());
        for (i, c) in commits.iter().enumerate() {
            println!("{}. {}", i + 1, c.title);
            if let Some(body) = &c.body { if !body.trim().is_empty() { println!("\n{}\n", body.trim()); } }
            println!("Files ({}):", c.files.len());
            for f in &c.files { println!("  - {}", f); }
            println!("");
        }
        if !leftovers.is_empty() {
            println!("Leftover files not in any commit ({}):", leftovers.len());
            for f in leftovers { println!("  - {}", f); }
            println!("");
        }
    } else {
        println!("No sensible multi-commit split proposed (showing single commit only).\n");
    }

    // Choose
    let choice = prompt_choice_keypress(if has_sensible_multi { "Choose [a] single, [b] multi, or [c] cancel: " } else { "Choose [a] single or [c] cancel: " }, if has_sensible_multi { &['a','b','c'] } else { &['a','c'] })?;
    match choice {
        'a' => {
            // Directly commit without extra confirmation
            run_in_repo(repo_dir, &["git", "add", "-A"])?;
            if let Some((subject, body)) = split_subject_body(&single_msg) {
                if body.trim().is_empty() {
                    run_in_repo(repo_dir, &["git", "commit", "-m", subject.trim()])?;
                } else {
                    run_in_repo(repo_dir, &["git", "commit", "-m", subject.trim(), "-m", body.trim()])?;
                }
            } else {
                run_in_repo(repo_dir, &["git", "commit", "-m", single_msg.trim()])?;
            }
            println!("Committed with AI message.");

            // Check for leftover changes and offer AI commit
            let leftovers = list_changed_files_vs_head(repo_dir)?;
            if !leftovers.is_empty() {
                println!("There are leftover uncommitted files ({}).", leftovers.len());
                for f in &leftovers { println!("  - {}", f); }
                if prompt_yes_no("Generate AI commit for leftovers? [y/N] ")? {
                    commit_files_with_ai(repo_dir, &leftovers, multi_progress)?;
                    println!("Leftover files committed.");
                }
            }
            Ok(())
        }
        'b' if has_sensible_multi => {
            let (commits, leftovers) = multi_plan.unwrap();
            // Directly execute multi-commit plan
            do_commits(repo_dir, &commits, &leftovers)?;
            // After executing plan, if leftovers remain (e.g., files added during run), offer AI commit
            let post_leftovers = if leftovers.is_empty() { list_changed_files_vs_head(repo_dir)? } else { leftovers.clone() };
            if !post_leftovers.is_empty() {
                println!("There are leftover uncommitted files ({}).", post_leftovers.len());
                for f in &post_leftovers { println!("  - {}", f); }
                if prompt_yes_no("Generate AI commit for leftovers? [y/N] ")? {
                    commit_files_with_ai(repo_dir, &post_leftovers, multi_progress)?;
                    println!("Leftover files committed.");
                }
            }
            println!("Multi-commit completed.");
            Ok(())
        }
        _ => { println!("Canceled."); Ok(()) }
    }
}

fn commit_with_ai_single(repo_dir: &Path, multi_progress: &MultiProgress) -> Result<()> {
    if !repo_dir.join(".git").exists() {
        println!("Not a git repository: {}", repo_dir.display());
        return Ok(());
    }
    let status_porcelain = run_in_repo(repo_dir, &["git", "status", "--porcelain"])?;
    if status_porcelain.trim().is_empty() {
        println!("No changes detected. Nothing to commit.");
        return Ok(());
    }

    let pb = multi_progress.add(ProgressBar::new_spinner());
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg} [{elapsed_precise}]").unwrap());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb.set_message("Generating single-commit proposal...");
    let name_status = run_in_repo(repo_dir, &["git", "diff", "--name-status", "HEAD"])?;
    let shortstat = run_in_repo(repo_dir, &["git", "diff", "--shortstat", "HEAD"])?;
    let diff_sample = truncate(&run_in_repo(repo_dir, &["git", "diff", "-U3", "HEAD"])? , 20_000);
    let prompt = build_commit_prompt_multiline(&name_status, &shortstat, &diff_sample);
    let msg = match generate_commit_message_via_gemini(&prompt) {
        Ok(m) => m,
        Err(_) => fallback_commit_message_multiline(&name_status, &shortstat),
    };
    pb.finish_with_message("Single-commit proposal ready");

    run_in_repo(repo_dir, &["git", "add", "-A"]) ?;
    if let Some((subject, body)) = split_subject_body(&msg) {
        if body.trim().is_empty() {
            run_in_repo(repo_dir, &["git", "commit", "-m", subject.trim()])?;
        } else {
            run_in_repo(repo_dir, &["git", "commit", "-m", subject.trim(), "-m", body.trim()])?;
        }
    } else {
        run_in_repo(repo_dir, &["git", "commit", "-m", msg.trim()])?;
    }
    println!("Committed with AI message.");

    let leftovers = list_changed_files_vs_head(repo_dir)?;
    if !leftovers.is_empty() {
        println!("There are leftover uncommitted files ({}).", leftovers.len());
        for f in &leftovers { println!("  - {}", f); }
        if prompt_yes_no("Generate AI commit for leftovers? [y/N] ")? {
            commit_files_with_ai(repo_dir, &leftovers, multi_progress)?;
            println!("Leftover files committed.");
        }
    }
    Ok(())
}

fn commit_with_ai_multi(repo_dir: &Path, multi_progress: &MultiProgress) -> Result<()> {
    if !repo_dir.join(".git").exists() {
        println!("Not a git repository: {}", repo_dir.display());
        return Ok(());
    }
    let status_porcelain = run_in_repo(repo_dir, &["git", "status", "--porcelain"])?;
    if status_porcelain.trim().is_empty() {
        println!("No changes detected. Nothing to commit.");
        return Ok(());
    }

    let pb = multi_progress.add(ProgressBar::new_spinner());
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg} [{elapsed_precise}]").unwrap());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb.set_message("Analyzing multi-commit plan...");
    let (commits, leftovers) = plan_multi_commits(repo_dir, multi_progress)?;
    pb.finish_with_message("Multi-commit analysis complete");

    println!("Proposed multi-commit plan:\n");
    for (i, c) in commits.iter().enumerate() {
        println!("{}. {}", i + 1, c.title);
        if let Some(body) = &c.body { if !body.trim().is_empty() { println!("\n{}\n", body.trim()); } }
        println!("Files ({}):", c.files.len());
        for f in &c.files { println!("  - {}", f); }
        println!("");
    }
    if !leftovers.is_empty() {
        println!("Leftover files not in any commit ({}):", leftovers.len());
        for f in &leftovers { println!("  - {}", f); }
        println!("");
    }
    if !prompt_yes_no(&format!("Proceed to create {} commits? [y/N] ", commits.len()))? {
        println!("Multi-commit canceled.");
        return Ok(());
    }
    do_commits(repo_dir, &commits, &leftovers)?;

    let post_leftovers = list_changed_files_vs_head(repo_dir)?;
    if !post_leftovers.is_empty() {
        println!("There are leftover uncommitted files ({}).", post_leftovers.len());
        for f in &post_leftovers { println!("  - {}", f); }
        if prompt_yes_no("Generate AI commit for leftovers? [y/N] ")? {
            commit_files_with_ai(repo_dir, &post_leftovers, multi_progress)?;
            println!("Leftover files committed.");
        }
    }
    println!("Multi-commit completed.");
    Ok(())
}

fn run_in_repo(repo_dir: &Path, args: &[&str]) -> Result<String> {
    let (cmd, rest) = args.split_first().ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let output = Command::new(cmd)
        .args(rest)
        .current_dir(repo_dir)
        .output()
        .with_context(|| format!("failed to run {:?}", args))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(anyhow::anyhow!(
            "command {:?} failed: {}",
            args, stderr.trim()
        ))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { format!("{}\n…[truncated]", &s[..max]) }
}

fn prompt_yes_no(prompt: &str) -> Result<bool> {
    use std::io::{self, Write};
    print!("{}", prompt);
    io::stdout().flush().ok();
    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(|e| anyhow::anyhow!("failed to read input: {}", e))?;
    let resp = input.trim().to_lowercase();
    Ok(resp == "y" || resp == "yes")
}

fn prompt_choice_keypress(prompt: &str, allowed: &[char]) -> Result<char> {
    use std::io::Write;
    print!("{}", prompt);
    std::io::stdout().flush().ok();
    terminal::enable_raw_mode().map_err(|e| anyhow::anyhow!("failed to enable raw mode: {}", e))?;
    let res = loop {
        match read() {
            Ok(Event::Key(key)) => match key.code {
                KeyCode::Char(c) => {
                    let cl = c.to_ascii_lowercase();
                    if allowed.contains(&cl) {
                        // echo selection and newline for feedback
                        print!("{}\n", c);
                        std::io::stdout().flush().ok();
                        break Ok(cl);
                    }
                }
                KeyCode::Esc => break Ok('c'),
                KeyCode::Enter => { /* ignore */ }
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(anyhow::anyhow!("failed to read key: {}", e)),
        }
    };
    terminal::disable_raw_mode().ok();
    res
}

fn split_subject_body(msg: &str) -> Option<(String, String)> {
    let mut lines = msg.lines();
    let subject = lines.next()?.to_string();
    let rest: String = lines.collect::<Vec<&str>>().join("\n");
    Some((subject, rest))
}

fn build_commit_prompt_multiline(name_status: &str, shortstat: &str, diff_sample: &str) -> String {
    format!(
        "You write excellent Conventional Commits. Generate a concise, multi-line commit message:\n\
        - First line: <type>(optional-scope): <summary> (<=72 chars, no trailing period)\n\
        - Blank line\n\
        - Body: 3-6 bullets summarizing key changes and rationale; wrap to ~72 chars\n\
        - Include 'BREAKING CHANGE:' line if applicable\n\
        Prefer specific wording over generic 'update' or 'changes'.\n\
        Changed files (name-status):\n\
        {}\n\
        Summary: {}\n\
        Diff sample (truncated):\n\
        {}\n\
        Output ONLY the commit message text.",
        name_status.trim(),
        shortstat.trim(),
        diff_sample.trim()
    )
}

fn fallback_commit_message_multiline(name_status: &str, shortstat: &str) -> String {
    // Simple heuristic fallback if API not available (multi-line)
    let files: Vec<&str> = name_status
        .lines()
        .take(5)
        .map(|l| l.split_whitespace().last().unwrap_or(l))
        .collect();
    let files_str = files.join(", ");
    let stat = shortstat.trim();
    let subject = if files_str.is_empty() { "chore: update files".to_string() } else { truncate(&format!("chore: update {}", files_str), 72) };
    let body = format!("\n\n- Update files\n- Summary: {}", if stat.is_empty() { "n/a" } else { stat });
    format!("{}{}", subject, body)
}

#[derive(Serialize)]
struct GeminiRequest<'a> {
    contents: Vec<GeminiContent<'a>>,
}

#[derive(Serialize)]
struct GeminiContent<'a> {
    parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize)]
struct GeminiPart<'a> { text: &'a str }

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,    
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiGeneratedContent>,
}

#[derive(Deserialize)]
struct GeminiGeneratedContent {
    parts: Option<Vec<GeminiGeneratedPart>>,   
}

#[derive(Deserialize)]
struct GeminiGeneratedPart { text: Option<String> }

fn generate_commit_message_via_gemini(prompt: &str) -> Result<String> {
    let api_key = std::env::var("GEMINI_API_KEY").map_err(|_| anyhow::anyhow!("GEMINI_API_KEY not set"))?;
    let model = "gemini-2.5-flash"; // updated model
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let req = GeminiRequest { contents: vec![GeminiContent { parts: vec![GeminiPart { text: prompt }] }] };
    let resp: GeminiResponse = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(serde_json::to_value(&req)?)
        .map_err(|e| anyhow::anyhow!("Gemini request failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow::anyhow!("invalid Gemini JSON: {}", e))?;

    let text = resp
        .candidates
        .and_then(|mut v| v.pop())
        .and_then(|c| c.content)
        .and_then(|c| c.parts)
        .and_then(|mut parts| parts.pop())
        .and_then(|p| p.text)
        .unwrap_or_default()
        .trim()
        .to_string();
    if text.is_empty() { anyhow::bail!("empty response from model") } else { Ok(text) }
}

// -------- Multi-commit planning --------

#[derive(Debug, Deserialize)]
struct CommitPlanResponse {
    commits: Vec<CommitPlan>,
}

#[derive(Debug, Deserialize)]
struct CommitPlan {
    title: String,
    body: Option<String>,
    files: Vec<String>,
}

fn plan_multi_commits(repo_dir: &Path, _multi_progress: &MultiProgress) -> Result<(Vec<CommitPlan>, Vec<String>)> {
    // Ensure repo and changes
    if !repo_dir.join(".git").exists() {
        anyhow::bail!("Not a git repository: {}", repo_dir.display());
    }
    let status_porcelain = run_in_repo(repo_dir, &["git", "status", "--porcelain"])?;
    if status_porcelain.trim().is_empty() {
        anyhow::bail!("no changes to commit");
    }

    // Gather change context
    let name_status = run_in_repo(repo_dir, &["git", "diff", "--name-status", "HEAD"])?;
    let numstat = run_in_repo(repo_dir, &["git", "diff", "--numstat", "HEAD"])?;
    let shortstat = run_in_repo(repo_dir, &["git", "diff", "--shortstat", "HEAD"])?;
    let diff_sample = truncate(&run_in_repo(repo_dir, &["git", "diff", "-U3", "HEAD"])? , 40_000);

    let plan_prompt = build_multi_commit_prompt(&name_status, &numstat, &shortstat, &diff_sample);
    let plan = match generate_commit_plan_via_gemini(&plan_prompt) {
        Ok(p) => p,
        Err(e) => {
            return Err(anyhow::anyhow!("AI planning failed: {}", e));
        }
    };

    // Collect actually changed files for validation
    let changed_files: Vec<String> = name_status
        .lines()
        .filter_map(|l| l.split_whitespace().nth(1))
        .map(|s| s.to_string())
        .collect();

    // Validate and normalize plan
    let mut normalized: Vec<CommitPlan> = Vec::new();
    for mut c in plan.commits {
        c.files.retain(|f| changed_files.iter().any(|cf| cf == f));
        if !c.title.trim().is_empty() && !c.files.is_empty() {
            normalized.push(c);
        }
    }

    if normalized.is_empty() {
        anyhow::bail!("AI did not propose any valid commits");
    }

    // Determine leftovers
    let mut included = std::collections::HashSet::new();
    for c in &normalized { for f in &c.files { included.insert(f.clone()); } }
    let leftovers: Vec<String> = changed_files
        .into_iter()
        .filter(|f| !included.contains(f))
        .collect();

    Ok((normalized, leftovers))
}

fn do_commits(repo_dir: &Path, commits: &Vec<CommitPlan>, _leftovers: &Vec<String>) -> Result<()> {
    // Execute commits in order
    for c in commits {
        let mut args = vec!["git", "add", "-A", "--"]; // stage specific files
        for f in &c.files { args.push(f); }
        run_in_repo(repo_dir, &args)?;

        let subject = c.title.trim();
        let body = c.body.as_deref().unwrap_or("").trim();
        let commit_res = if body.is_empty() {
            run_in_repo(repo_dir, &["git", "commit", "-m", subject])
        } else {
            run_in_repo(repo_dir, &["git", "commit", "-m", subject, "-m", body])
        };
        if let Err(e) = commit_res { return Err(e); }
    }

    // Leave handling of leftovers to the caller (they may choose AI commit)
    Ok(())
}

fn build_multi_commit_prompt(name_status: &str, numstat: &str, shortstat: &str, diff_sample: &str) -> String {
    format!(
        "Analyze the following changes and propose a set of logical commits.\n\
        Output STRICT JSON with this schema: {{\"commits\":[{{\"title\":string,\"body\":string,\"files\":[string]}}]}}.\n\
        Rules:\n\
        - Group changes by intent/scope so each commit is meaningful.\n\
        - Use Conventional Commit titles (<=72 chars).\n\
        - Body should briefly explain rationale and key changes (optional).\n\
        - Assign each changed file to at most one commit.\n\
        Changed files (name-status):\n{}\n\
        Per-file stats (numstat):\n{}\n\
        Summary: {}\n\
        Diff sample (truncated):\n{}\n\
        JSON only.",
        name_status.trim(), numstat.trim(), shortstat.trim(), diff_sample.trim()
    )
}

fn generate_commit_plan_via_gemini(prompt: &str) -> Result<CommitPlanResponse> {
    let api_key = std::env::var("GEMINI_API_KEY").map_err(|_| anyhow::anyhow!("GEMINI_API_KEY not set"))?;
    let model = "gemini-2.5-flash";
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let req = GeminiRequest { contents: vec![GeminiContent { parts: vec![GeminiPart { text: prompt }] }] };
    let resp: GeminiResponse = ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(serde_json::to_value(&req)?)
        .map_err(|e| anyhow::anyhow!("Gemini request failed: {}", e))?
        .into_json()
        .map_err(|e| anyhow::anyhow!("invalid Gemini JSON: {}", e))?;

    let text = resp
        .candidates
        .and_then(|mut v| v.pop())
        .and_then(|c| c.content)
        .and_then(|c| c.parts)
        .and_then(|mut parts| parts.pop())
        .and_then(|p| p.text)
        .ok_or_else(|| anyhow::anyhow!("empty model response"))?;

    // Attempt to parse the returned text as JSON plan
    let plan: CommitPlanResponse = serde_json::from_str(text.trim())
        .map_err(|e| anyhow::anyhow!("failed to parse plan JSON: {}", e))?;
    Ok(plan)
}

// -------- Leftover helpers --------

fn list_changed_files_vs_head(repo_dir: &Path) -> Result<Vec<String>> {
    let out = run_in_repo(repo_dir, &["git", "diff", "--name-only", "HEAD"])?;
    let files: Vec<String> = out
        .lines()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    Ok(files)
}

fn run_in_repo_strings(repo_dir: &Path, args: Vec<String>) -> Result<String> {
    let mut it = args.iter();
    let cmd = it.next().ok_or_else(|| anyhow::anyhow!("empty command"))?;
    let output = Command::new(OsStr::new(cmd))
        .args(&args[1..])
        .current_dir(repo_dir)
        .output()
        .with_context(|| format!("failed to run {:?}", args))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(anyhow::anyhow!("command {:?} failed: {}", args, stderr.trim()))
    }
}

fn diff_context_for_files(repo_dir: &Path, files: &Vec<String>) -> Result<(String, String, String)> {
    let mut name_status_args = vec!["git".to_string(), "diff".to_string(), "--name-status".to_string(), "HEAD".to_string(), "--".to_string()];
    let mut shortstat_args = vec!["git".to_string(), "diff".to_string(), "--shortstat".to_string(), "HEAD".to_string(), "--".to_string()];
    let mut diff_args = vec!["git".to_string(), "diff".to_string(), "-U3".to_string(), "HEAD".to_string(), "--".to_string()];
    for f in files { name_status_args.push(f.clone()); shortstat_args.push(f.clone()); diff_args.push(f.clone()); }
    let name_status = run_in_repo_strings(repo_dir, name_status_args)?;
    let shortstat = run_in_repo_strings(repo_dir, shortstat_args)?;
    let diff_sample = truncate(&run_in_repo_strings(repo_dir, diff_args)?, 20_000);
    Ok((name_status, shortstat, diff_sample))
}

fn commit_files_with_ai(repo_dir: &Path, files: &Vec<String>, multi_progress: &MultiProgress) -> Result<()> {
    if files.is_empty() { return Ok(()); }
    let pb = multi_progress.add(ProgressBar::new_spinner());
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg} [{elapsed_precise}]").unwrap());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb.set_message("Generating commit for leftovers...");

    let (name_status, shortstat, diff_sample) = diff_context_for_files(repo_dir, files)?;
    let prompt = build_commit_prompt_multiline(&name_status, &shortstat, &diff_sample);
    let msg = match generate_commit_message_via_gemini(&prompt) {
        Ok(m) => m,
        Err(_) => fallback_commit_message_multiline(&name_status, &shortstat),
    };
    pb.finish_with_message("Leftover commit proposal ready");

    // Stage only these files and commit
    let mut add_args = vec!["git".to_string(), "add".to_string(), "-A".to_string(), "--".to_string()];
    for f in files { add_args.push(f.clone()); }
    run_in_repo_strings(repo_dir, add_args)?;

    if let Some((subject, body)) = split_subject_body(&msg) {
        if body.trim().is_empty() {
            run_in_repo(repo_dir, &["git", "commit", "-m", subject.trim()])?;
        } else {
            run_in_repo(repo_dir, &["git", "commit", "-m", subject.trim(), "-m", body.trim()])?;
        }
    } else {
        run_in_repo(repo_dir, &["git", "commit", "-m", msg.trim()])?;
    }
    Ok(())
}

fn is_text_file(path: &Path, repo_types: Option<&[RepoType]>) -> Result<bool> {
    // First check the path against excluded patterns
    let path_str = path.to_string_lossy();
    if EXCLUDED_PATTERNS.iter().any(|pattern| path_str.contains(pattern)) {
        return Ok(false);
    }

    // Always allow README files
    let file_name = path_str.to_lowercase();
    if file_name.contains("readme.") || file_name == "readme" {
        return Ok(true);
    }

    // If repo_types is specified, check if file matches any of the types
    if let Some(repo_types) = repo_types {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            return Ok(
                repo_types
                    .iter()
                    .any(|repo_type| {
                        get_repo_type_extensions(repo_type).contains(&ext_str.as_str())
                    })
            );
        }
        return Ok(false);
    }

    // If no repo_types specified, use the original text file detection logic
    // Check if it's a known text extension
    if let Some(ext) = path.extension() {
        let ext_str = ext.to_string_lossy().to_lowercase();
        if TEXT_EXTENSIONS.contains(&ext_str.as_str()) {
            return Ok(true);
        }
    }

    // Use file signature detection
    if let Some(kind) = infer::get_from_path(path)? {
        let mime = kind.mime_type();
        // Known text MIME types
        if mime.starts_with("text/") || mime == "application/json" || mime == "application/xml" {
            return Ok(true);
        }
        // Known binary MIME types
        if
            mime.starts_with("image/") ||
            mime.starts_with("audio/") ||
            mime.starts_with("video/") ||
            mime.starts_with("application/octet-stream") ||
            mime.starts_with("application/x-executable")
        {
            return Ok(false);
        }
    }

    // If we can't determine by MIME type, analyze content
    let mut file = File::open(path)?;
    let mut buffer = vec![0; BINARY_CHECK_SIZE];
    let n = file.read(&mut buffer)?;
    if n == 0 {
        return Ok(true); // Empty files are considered text
    }

    // Count control characters and high ASCII
    let non_text = buffer[..n]
        .iter()
        .filter(|&&byte| {
            // Allow common control chars: tab, newline, carriage return
            byte != b'\t' &&
                byte != b'\n' &&
                byte != b'\r' &&
                // Consider control characters and high ASCII as non-text
                (byte < 32 || byte > 126)
        })
        .count();

    // Calculate ratio of non-text bytes
    let ratio = (non_text as f32) / (n as f32);
    Ok(ratio <= TEXT_THRESHOLD)
}

fn should_process_file(path: &Path, repo_types: Option<&[RepoType]>, only_patterns: &[String]) -> bool {
    // If --only patterns are specified, check against them first
    if !only_patterns.is_empty() {
        let path_str = path.to_string_lossy();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        
        // Check if any pattern matches the full path or just the filename
        let matches_pattern = only_patterns.iter().any(|pattern| {
            if let Ok(glob_pattern) = Pattern::new(pattern) {
                glob_pattern.matches(&path_str) || glob_pattern.matches(file_name)
            } else {
                false
            }
        });
        
        if !matches_pattern {
            return false;
        }
    }
    
    // If --only patterns match or are not specified, continue with regular filtering
    match is_text_file(path, repo_types) {
        Ok(is_text) => is_text,
        Err(_) => false,
    }
}


fn extract_repo_name(url: &str) -> String {
    url.split('/').last().unwrap_or("repo").trim_end_matches(".git").to_string()
}

fn is_binary_file(path: &Path) -> Result<bool> {
    // First check if we can detect the file type
    if let Some(kind) = infer::get_from_path(path)? {
        return Ok(!kind.mime_type().starts_with("text/"));
    }

    // If we can't detect the type, try to read the first few bytes
    // to check for null bytes (common in binary files)
    let mut file = File::open(path)?;
    let mut buffer = [0; 512];
    let n = file.read(&mut buffer)?;

    // Check for null bytes in the first chunk of the file
    Ok(buffer[..n].contains(&0))
}

fn print_stats(stats: &ProcessingStats) {
    println!("\nProcessing Statistics:");
    println!("Total repositories processed: {}", stats.repo_count);
    println!("Total files processed: {}", stats.total_files);
    println!("Total binary files skipped: {}", stats.binary_files_skipped);
    println!("Total tokens: {}", stats.total_tokens);
    println!("Repository clone time: {:.2} seconds", stats.clone_time);
    println!("Content processing time: {:.2} seconds", stats.processing_time);
    println!("Total time: {:.2} seconds", stats.clone_time + stats.processing_time);
    println!(
        "Average tokens per file: {:.2}",
        (stats.total_tokens as f64) / (stats.total_files as f64)
    );
    println!(
        "Processing speed: {:.2} files/second",
        (stats.total_files as f64) / stats.processing_time
    );
}
