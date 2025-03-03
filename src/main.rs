use std::{
    fs::{ self, File },
    io::{ Write, BufReader, Read },
    path::Path,
    time::Instant,
    sync::Arc,
    path::PathBuf,
};
use anyhow::{ Context, Result };
use clap::Parser;
use git2::Repository;
use tempfile::TempDir;
use walkdir::WalkDir;
use tiktoken_rs::o200k_base;
use chrono::Local;
use parking_lot::Mutex;
use rayon::prelude::*;
use memmap2::Mmap;
use infer;
use dirs;
use copypasta::{ ClipboardContext, ClipboardProvider };
use indicatif::{ ProgressBar, ProgressStyle, MultiProgress, ParallelProgressIterator };

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
    ".tiktoken",
    ".bin",
    ".pack",
    ".idx",
    ".cache",
    "package-lock.json",
    "yarn.lock",
    "Cargo.lock",
    "venv/",
    ".venv/",
    "env/",
    "__pycache__/",
    ".pytest_cache/",
    ".svn/",
    ".hg/",
    ".DS_Store",
    ".idea/",
    ".vs/",
    ".vscode/",
    ".gradle/",
    "out/",
    "coverage/",
    "tmp/",
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

    /// Copy output to clipboard instead of saving to file
    #[arg(long)]
    copy: bool,
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

    // Only create output directory if we're not copying to clipboard
    if !args.copy {
        fs::create_dir_all(&args.output_dir)?;
    }

    // Process repositories in parallel if there are multiple
    if urls.len() > 1 {
        urls
            .par_iter()
            .try_for_each(|url| {
                process_repository(
                    url,
                    &args.output_dir,
                    Arc::clone(&stats),
                    &args,
                    Arc::clone(&multi_progress)
                )
            })?;
    } else {
        process_repository(
            &urls[0],
            &args.output_dir,
            Arc::clone(&stats),
            &args,
            Arc::clone(&multi_progress)
        )?;
    }

    let final_stats = stats.lock();
    print_stats(&final_stats);
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
        let mut content = String::with_capacity(metadata.len() as usize);
        BufReader::new(file).read_to_string(&mut content)?;
        Ok(content)
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

    // Count total files first for progress bar
    let total_files = WalkDir::new(&repo_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
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
    let files: Vec<_> = WalkDir::new(&repo_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .par_bridge()
        .progress_with(process_pb.clone())
        .filter_map(|entry| {
            let path = entry.path();
            // Skip if this is the README we already processed
            if let Some(ref readme) = readme_content {
                if path.file_name().and_then(|n| n.to_str()) == Some(&readme.path) {
                    return None;
                }
            }

            if
                !should_process_file(path, if args.repo_types.is_empty() {
                    None
                } else {
                    Some(&args.repo_types)
                }) ||
                matches!(is_binary_file(path), Ok(true))
            {
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
    let tree = DirectoryTree::build(&repo_dir, EXCLUDED_PATTERNS)?;
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
    if args.copy {
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

fn should_process_file(path: &Path, repo_types: Option<&[RepoType]>) -> bool {
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
